"""Чтение отчётов из общего volume.

Сервис монтирует volume только на чтение и ничего в него не пишет.
Отчёты появляются в outbox/ через атомарный rename (см. processor/src/spool.rs),
поэтому полузаписанный файл прочитать невозможно; временные файлы начинаются
с точки и пропускаются.
"""

from __future__ import annotations

import logging
import re
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import TypeVar

from pydantic import BaseModel, ValidationError

from .models import FailedBatch, Report

log = logging.getLogger("reports.store")

M = TypeVar("M", bound=BaseModel)

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


class InvalidFileError(Exception):
    """Файл есть, но не соответствует формату отчёта."""


# Результат разбора файла: модель или признак повреждения (тоже кешируется,
# чтобы испорченный файл не перечитывался и не засорял лог на каждом запросе).
_Parsed = BaseModel | InvalidFileError


class ReportStore:
    """Каталог отчётов с кешем разобранных файлов: повторно читается только изменённое."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self._cache: dict[Path, tuple[_CacheKey, _Parsed]] = {}
        self._lock = threading.Lock()

    def readable(self) -> bool:
        return (self.root / OUTBOX).is_dir()

    def _load(self, path: Path, model: type[M]) -> M | InvalidFileError | None:
        """Модель из файла; InvalidFileError — файл повреждён; None — файла нет."""
        try:
            st = path.stat()
        except FileNotFoundError:
            return None
        key = _CacheKey(st.st_mtime_ns, st.st_size)
        with self._lock:
            cached = self._cache.get(path)
            if cached and cached[0] == key:
                return cached[1]  # type: ignore[return-value]
        parsed: M | InvalidFileError
        try:
            parsed = model.model_validate_json(path.read_bytes())
        except FileNotFoundError:
            return None  # удалён между stat и чтением
        except (OSError, ValidationError) as exc:
            log.warning("пропущен некорректный файл %s/%s: %s", path.parent.name, path.name, _short(exc))
            parsed = InvalidFileError(str(exc))
        with self._lock:
            self._cache[path] = (key, parsed)
        return parsed

    def _scan(self, subdir: str, model: type[M]) -> tuple[list[M], int]:
        """Корректные записи каталога и число пропущенных повреждённых файлов."""
        directory = self.root / subdir
        if not directory.is_dir():
            return [], 0
        files = [p for p in directory.iterdir() if p.suffix == ".json" and not p.name.startswith(".")]
        items: list[M] = []
        invalid = 0
        for p in files:
            parsed = self._load(p, model)
            if isinstance(parsed, InvalidFileError):
                invalid += 1
            elif parsed is not None:
                items.append(parsed)
        # Удалённые файлы убираем из кеша, чтобы он не рос бесконечно.
        with self._lock:
            for stale in [p for p in self._cache if p.parent == directory and p not in files]:
                del self._cache[stale]
        return items, invalid

    def reports(self, name: str | None = None) -> tuple[list[Report], int]:
        items, invalid = self._scan(OUTBOX, Report)
        if name is not None:
            items = [r for r in items if r.name == name]
        items.sort(key=lambda r: r.processed_at, reverse=True)
        return items, invalid

    def report(self, report_id: str) -> Report | None:
        """Отчёт по id; None — нет такого; InvalidFileError — файл повреждён."""
        if not valid_id(report_id):
            return None
        parsed = self._load(self.root / OUTBOX / f"{report_id}.json", Report)
        if isinstance(parsed, InvalidFileError):
            raise parsed
        return parsed

    def failed(self) -> tuple[list[FailedBatch], int]:
        items, invalid = self._scan(FAILED, FailedBatch)
        items.sort(key=lambda r: r.failed_at, reverse=True)
        return items, invalid


def _short(exc: Exception) -> str:
    if isinstance(exc, ValidationError):
        return "; ".join(f"{'.'.join(map(str, e['loc']))}: {e['msg']}" for e in exc.errors()[:3])
    return str(exc)
