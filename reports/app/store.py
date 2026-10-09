"""Чтение отчётов из общего volume.

Сервис монтирует volume только на чтение и ничего в него не пишет.
Отчёты появляются в outbox/ через атомарный rename (см. processor/src/spool.rs),
поэтому полузаписанный файл прочитать невозможно; временные файлы начинаются
с точки и пропускаются.
"""

from __future__ import annotations

import json
import logging
import re
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any

log = logging.getLogger("reports.store")

OUTBOX = "outbox"
FAILED = "failed"

# id — UUID от ingest. Строгая проверка заодно исключает обход каталога (../../etc/passwd).
_ID_PATTERN = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


def valid_id(report_id: str) -> bool:
    return bool(_ID_PATTERN.fullmatch(report_id))


@dataclass(frozen=True)
class _CacheKey:
    mtime_ns: int
    size: int


class ReportStore:
    """Каталог отчётов с кешем разобранных файлов: повторно читается только изменённое."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self._cache: dict[Path, tuple[_CacheKey, dict[str, Any]]] = {}
        self._lock = threading.Lock()

    def readable(self) -> bool:
        return (self.root / OUTBOX).is_dir()

    def _load(self, path: Path) -> dict[str, Any] | None:
        try:
            st = path.stat()
        except FileNotFoundError:
            return None
        key = _CacheKey(st.st_mtime_ns, st.st_size)
        with self._lock:
            cached = self._cache.get(path)
            if cached and cached[0] == key:
                return cached[1]
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            log.warning("пропущен некорректный файл %s: %s", path.name, exc)
            return None
        with self._lock:
            self._cache[path] = (key, data)
        return data

    def _scan(self, subdir: str) -> list[dict[str, Any]]:
        directory = self.root / subdir
        if not directory.is_dir():
            return []
        files = [p for p in directory.iterdir() if p.suffix == ".json" and not p.name.startswith(".")]
        items = [d for p in files if (d := self._load(p)) is not None]
        # Удалённые файлы убираем из кеша, чтобы он не рос бесконечно.
        with self._lock:
            for stale in [p for p in self._cache if p.parent == directory and p not in files]:
                del self._cache[stale]
        return items

    def reports(self, name: str | None = None) -> list[dict[str, Any]]:
        items = self._scan(OUTBOX)
        if name is not None:
            items = [r for r in items if r.get("name") == name]
        items.sort(key=lambda r: r.get("processed_at", ""), reverse=True)
        return items

    def report(self, report_id: str) -> dict[str, Any] | None:
        if not valid_id(report_id):
            return None
        return self._load(self.root / OUTBOX / f"{report_id}.json")

    def failed(self) -> list[dict[str, Any]]:
        items = self._scan(FAILED)
        items.sort(key=lambda r: r.get("failed_at", ""), reverse=True)
        return items
