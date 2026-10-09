from __future__ import annotations

import json
import statistics
import uuid
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient

from app.main import create_app
from app.store import ReportStore


def make_report(name: str, values: list[float], processed_at: str) -> dict[str, Any]:
    """Отчёт в том формате, в каком его пишет processor (processor/src/main.rs)."""
    return {
        "id": str(uuid.uuid4()),
        "name": name,
        "received_at": "2026-10-09T10:00:00Z",
        "processed_at": processed_at,
        "duration_ms": 0.1,
        "processor": "rust-processor/0.1.0",
        "stats": {
            "count": len(values),
            "sum": sum(values),
            "mean": statistics.fmean(values),
            "min": min(values),
            "max": max(values),
            "median": statistics.median(values),
            "p95": max(values),
            "p99": max(values),
            "stddev": statistics.stdev(values) if len(values) > 1 else 0.0,
        },
        "histogram": {"min": min(values), "max": max(values), "counts": [len(values)]},
    }


@pytest.fixture
def data_dir(tmp_path: Path) -> Path:
    for d in ("outbox", "failed"):
        (tmp_path / d).mkdir()
    return tmp_path


def put(data_dir: Path, sub: str, doc: dict[str, Any], name: str | None = None) -> None:
    (data_dir / sub / (name or f"{doc['id']}.json")).write_text(json.dumps(doc), encoding="utf-8")


@pytest.fixture
def client(data_dir: Path) -> Iterator[TestClient]:
    with TestClient(create_app(data_dir)) as c:
        yield c


def test_list_and_get(client: TestClient, data_dir: Path) -> None:
    old = make_report("sensor-a", [1, 2, 3], "2026-10-09T10:00:01.000Z")
    new = make_report("sensor-b", [10, 20], "2026-10-09T10:00:02.000Z")
    put(data_dir, "outbox", old)
    put(data_dir, "outbox", new)
    put(data_dir, "outbox", {"partial": True}, name=".tmp-file.tmp")  # временный файл не виден

    body = client.get("/api/v1/reports").json()
    assert body["total"] == 2
    assert [i["id"] for i in body["items"]] == [new["id"], old["id"]]  # новые первыми
    assert body["items"][1]["mean"] == 2

    assert client.get("/api/v1/reports", params={"name": "sensor-a"}).json()["total"] == 1
    assert client.get("/api/v1/reports", params={"limit": 1, "offset": 1}).json()["items"][0]["id"] == old["id"]
    assert client.get(f"/api/v1/reports/{new['id']}").json()["stats"]["count"] == 2


def test_get_errors(client: TestClient) -> None:
    assert client.get(f"/api/v1/reports/{uuid.uuid4()}").status_code == 404
    assert client.get("/api/v1/reports/not-a-uuid").status_code == 400
    # обход каталога невозможен: id обязан быть UUID
    assert client.get("/api/v1/reports/..%2F..%2Fetc%2Fpasswd").status_code in (400, 404)
    assert client.get("/api/v1/reports", params={"limit": 0}).status_code == 422


def test_summary_and_failed(client: TestClient, data_dir: Path) -> None:
    put(data_dir, "outbox", make_report("a", [1, 2], "2026-10-09T10:00:01Z"))
    put(data_dir, "outbox", make_report("a", [3], "2026-10-09T10:00:02Z"))
    put(data_dir, "outbox", make_report("b", [4, 5, 6], "2026-10-09T10:00:03Z"))
    put(data_dir, "failed", {"id": str(uuid.uuid4()), "error": "пустой пакет", "failed_at": "2026-10-09T10:00:04Z"})

    s = client.get("/api/v1/summary").json()
    assert s == {"reports": 3, "failed": 1, "values": 6, "by_name": {"a": 2, "b": 1}, "invalid_files": 0}
    assert client.get("/api/v1/failed").json()[0]["error"] == "пустой пакет"


def _broken_reports() -> dict[str, Any]:
    with_null = make_report("x", [1], "2026-10-09T10:00:01Z")
    with_null["stats"]["mean"] = None  # так serde_json записывает NaN/inf
    no_stats = make_report("x", [1], "2026-10-09T10:00:01Z")
    del no_stats["stats"]
    wrong_type = make_report("x", [1], "2026-10-09T10:00:01Z")
    wrong_type["stats"]["count"] = "много"
    return {"null вместо числа": with_null, "нет stats": no_stats, "неверный тип": wrong_type}


@pytest.mark.parametrize("case", list(_broken_reports()))
def test_broken_report_does_not_break_listing(client: TestClient, data_dir: Path, case: str) -> None:
    broken = _broken_reports()[case]
    put(data_dir, "outbox", broken)
    put(data_dir, "outbox", make_report("ok", [1, 2], "2026-10-09T10:00:02Z"))
    (data_dir / "outbox" / f"{uuid.uuid4()}.json").write_text("{not json", encoding="utf-8")

    r = client.get("/api/v1/reports")
    assert r.status_code == 200, r.text  # раньше один такой файл давал 500 на всём списке
    assert r.json()["total"] == 1
    assert client.get("/api/v1/summary").json()["invalid_files"] == 2
    assert client.get(f"/api/v1/reports/{broken['id']}").status_code == 502


def test_extra_fields_are_ignored(client: TestClient, data_dir: Path) -> None:
    # «Терпимый читатель»: новое поле в отчёте processor не ломает reports.
    report = make_report("x", [1, 2], "2026-10-09T10:00:01Z")
    report["new_field"] = {"anything": 1}
    put(data_dir, "outbox", report)
    assert client.get("/api/v1/reports").json()["total"] == 1


def test_health(client: TestClient, data_dir: Path) -> None:
    assert client.get("/health/ready").status_code == 200
    (data_dir / "outbox").rmdir()
    assert client.get("/health/ready").status_code == 503
    assert client.get("/health/live").status_code == 200


def test_cache_rereads_changed_file(data_dir: Path) -> None:
    store = ReportStore(data_dir)
    report = make_report("x", [1], "2026-10-09T10:00:01Z")
    put(data_dir, "outbox", report)
    assert store.report(report["id"]).name == "x"

    report["name"] = "renamed-longer-name"  # другой размер файла -> кеш недействителен
    put(data_dir, "outbox", report)
    assert store.report(report["id"]).name == "renamed-longer-name"

    (data_dir / "outbox" / f"{report['id']}.json").unlink()
    assert store.reports() == ([], 0)
    assert store._cache == {}  # удалённый файл ушёл из кеша


def test_broken_file_logged_once(data_dir: Path, caplog: pytest.LogCaptureFixture) -> None:
    (data_dir / "outbox" / f"{uuid.uuid4()}.json").write_text("{not json", encoding="utf-8")
    store = ReportStore(data_dir)
    for _ in range(3):
        store.reports()
    # Повреждённый файл кешируется как повреждённый и не засоряет лог на каждом запросе.
    assert sum("пропущен некорректный файл" in m for m in caplog.messages) == 1
