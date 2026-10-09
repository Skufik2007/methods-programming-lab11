"""Точка входа: python -m app."""

from __future__ import annotations

import logging
import os

import uvicorn

from .main import create_app


def main() -> None:
    level = os.getenv("LOG_LEVEL", "info").lower()
    logging.basicConfig(level=level.upper(), format="%(asctime)s %(levelname)s %(name)s %(message)s")
    # uvicorn сам обрабатывает SIGTERM: дожидается активных запросов и выходит.
    uvicorn.run(
        create_app(),
        host=os.getenv("HOST", "0.0.0.0"),  # noqa: S104 — сервис работает в контейнере
        port=int(os.getenv("PORT", "8000")),
        log_level=level,
        timeout_graceful_shutdown=10,
    )


if __name__ == "__main__":
    main()
