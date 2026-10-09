"""Формат файлов в общем volume — контракт между processor (Rust) и reports (Python).

Каждый файл из outbox/ и failed/ проверяется этими моделями при чтении. Файл,
который им не соответствует (старый формат, ручная правка, null вместо числа),
пропускается с предупреждением в логе и не ломает ответы сервиса.

Модели следуют принципу «терпимого читателя»: незнакомые поля игнорируются,
поэтому новое поле в отчёте processor не ломает reports.
"""

from __future__ import annotations

from typing import Annotated

from pydantic import BaseModel, ConfigDict, Field

# Числа статистики обязаны быть конечными: null, inf и NaN — признак испорченного отчёта.
Finite = Annotated[float, Field(allow_inf_nan=False)]


class _Contract(BaseModel):
    model_config = ConfigDict(extra="ignore", frozen=True)


class Stats(_Contract):
    count: Annotated[int, Field(ge=1)]
    sum: Finite
    mean: Finite
    min: Finite
    max: Finite
    median: Finite
    p95: Finite
    p99: Finite
    stddev: Annotated[float, Field(ge=0, allow_inf_nan=False)]


class Histogram(_Contract):
    min: Finite
    max: Finite
    counts: list[Annotated[int, Field(ge=0)]]


class Report(_Contract):
    id: str
    name: str
    received_at: str
    processed_at: str
    duration_ms: Finite
    processor: str
    stats: Stats
    histogram: Histogram


class FailedBatch(_Contract):
    id: str
    error: str
    failed_at: str
