"""reports — Python-сервис (FastAPI): отдаёт отчёты, которые посчитал processor (Rust).

Swagger UI — /docs.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Annotated

from fastapi import FastAPI, HTTPException, Query
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

from .models import FailedBatch, Report
from .store import InvalidFileError, ReportStore, valid_id


class ReportBrief(BaseModel):
    id: str
    name: str
    processed_at: str
    count: int
    mean: float


class ReportList(BaseModel):
    total: int
    items: list[ReportBrief]


class Summary(BaseModel):
    reports: int
    failed: int
    values: int
    by_name: dict[str, int]
    invalid_files: int = Field(description="Повреждённые файлы в outbox/ и failed/, пропущенные при чтении")


def create_app(data_dir: Path | None = None) -> FastAPI:
    store = ReportStore(data_dir or Path(os.getenv("DATA_DIR", "/data")))
    app = FastAPI(
        title="Lab11 reports",
        description="Отчёты по пакетам измерений, посчитанные Rust-сервисом processor.",
        version="1.0.0",
    )

    @app.get("/health/live", tags=["service"])
    def live() -> dict[str, str]:
        return {"status": "ok"}

    @app.get("/health/ready", tags=["service"])
    def ready() -> JSONResponse:
        if not store.readable():
            return JSONResponse({"status": "volume_unavailable"}, status_code=503)
        return JSONResponse({"status": "ready"})

    @app.get("/api/v1/reports", response_model=ReportList, tags=["reports"])
    def list_reports(
        limit: Annotated[int, Query(ge=1, le=100)] = 20,
        offset: Annotated[int, Query(ge=0)] = 0,
        name: Annotated[str | None, Query(max_length=64)] = None,
    ) -> ReportList:
        items, _ = store.reports(name)
        page = items[offset : offset + limit]
        return ReportList(
            total=len(items),
            items=[
                ReportBrief(id=r.id, name=r.name, processed_at=r.processed_at, count=r.stats.count, mean=r.stats.mean)
                for r in page
            ],
        )

    @app.get(
        "/api/v1/reports/{report_id}",
        response_model=Report,
        tags=["reports"],
        responses={502: {"description": "Файл отчёта повреждён"}},
    )
    def get_report(report_id: str) -> Report:
        if not valid_id(report_id):
            raise HTTPException(400, "id должен быть UUID")
        try:
            report = store.report(report_id)
        except InvalidFileError as exc:
            # Данные испортил источник (processor или ручная правка volume), а не клиент.
            raise HTTPException(502, "файл отчёта повреждён и не соответствует формату") from exc
        if report is None:
            raise HTTPException(404, "отчёт не найден (пакет ещё обрабатывается или не существует)")
        return report

    @app.get("/api/v1/failed", response_model=list[FailedBatch], tags=["reports"])
    def failed() -> list[FailedBatch]:
        items, _ = store.failed()
        return items

    @app.get("/api/v1/summary", response_model=Summary, tags=["reports"])
    def summary() -> Summary:
        reports, bad_reports = store.reports()
        failed_items, bad_failed = store.failed()
        by_name: dict[str, int] = {}
        for r in reports:
            by_name[r.name] = by_name.get(r.name, 0) + 1
        return Summary(
            reports=len(reports),
            failed=len(failed_items),
            values=sum(r.stats.count for r in reports),
            by_name=by_name,
            invalid_files=bad_reports + bad_failed,
        )

    return app
