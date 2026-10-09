"""Сквозная проверка конвейера Go → volume → Rust → volume → Python (только стандартная библиотека).

    python deploy/e2e.py [--ingest http://localhost:8081] [--reports http://localhost:8082]

Отправляет пакеты в ingest, ждёт, пока processor их обработает, забирает отчёты
из reports и сверяет статистику с эталоном, посчитанным модулем statistics.
"""

from __future__ import annotations

import argparse
import json
import math
import random
import statistics
import sys
import time
import urllib.error
import urllib.request
from typing import Any


def call(method: str, url: str, body: Any = None) -> tuple[int, Any]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status, json.load(resp)
    except urllib.error.HTTPError as exc:
        return exc.code, json.load(exc)


def check(cond: bool, message: str) -> None:
    print(("OK   " if cond else "FAIL ") + message)
    if not cond:
        sys.exit(1)


def expected(values: list[float]) -> dict[str, float]:
    # method="inclusive" — линейная интерполяция, как в processor (и numpy.percentile).
    q = statistics.quantiles(values, n=100, method="inclusive")
    return {
        "count": len(values),
        "mean": statistics.fmean(values),
        "median": statistics.median(values),
        "p95": q[94],
        "p99": q[98],
        "stddev": statistics.stdev(values),
        "min": min(values),
        "max": max(values),
    }


def wait_done(ingest: str, batch_id: str, timeout: float = 30) -> str:
    deadline = time.monotonic() + timeout
    status = "unknown"
    while time.monotonic() < deadline:
        _, body = call("GET", f"{ingest}/api/v1/batches/{batch_id}")
        status = body.get("status", status)
        if status in ("done", "failed"):
            return status
        time.sleep(0.2)
    return status


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ingest", default="http://localhost:8081")
    parser.add_argument("--reports", default="http://localhost:8082")
    parser.add_argument("--batches", type=int, default=5)
    args = parser.parse_args()

    rng = random.Random(42)  # noqa: S311 — тестовые данные, не криптография
    sent: dict[str, list[float]] = {}
    for i in range(args.batches):
        values = [round(rng.gauss(100, 15), 3) for _ in range(rng.randint(10, 5000))]
        status, body = call("POST", f"{args.ingest}/api/v1/batches", {"name": f"sensor-{i % 2}", "values": values})
        check(status == 202, f"ingest принял пакет {i} из {len(values)} значений ({status})")
        sent[body["id"]] = values

    status, _ = call("POST", f"{args.ingest}/api/v1/batches", {"name": "bad name!", "values": [1]})
    check(status == 422, f"ingest отклонил некорректное имя ({status})")

    for batch_id, values in sent.items():
        final = wait_done(args.ingest, batch_id)
        check(final == "done", f"пакет {batch_id[:8]} обработан processor ({final})")

        status, report = call("GET", f"{args.reports}/api/v1/reports/{batch_id}")
        check(status == 200, f"reports отдал отчёт {batch_id[:8]} ({status})")
        want = expected(values)
        got = report["stats"]
        bad = {k: (got[k], v) for k, v in want.items() if not math.isclose(got[k], v, rel_tol=1e-9, abs_tol=1e-9)}
        check(not bad, f"статистика совпадает с эталоном Python {bad or ''}")
        check(sum(report["histogram"]["counts"]) == len(values), "гистограмма покрывает все значения")

    status, summary = call("GET", f"{args.reports}/api/v1/summary")
    check(status == 200 and summary["reports"] >= len(sent), f"сводка: {summary}")
    print("\nвсе проверки пройдены")


if __name__ == "__main__":
    main()
