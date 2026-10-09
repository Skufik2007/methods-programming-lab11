# syntax=docker/dockerfile:1

# ---- сборка: зависимости в отдельном venv ----
FROM python:3.12-slim AS build
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
RUN python -m venv /venv
# Слой зависимостей зависит только от requirements.txt; pip-кеш — в cache mount
# (повторная установка после смены одной версии не качает всё заново).
COPY requirements.txt /tmp/requirements.txt
RUN --mount=type=cache,target=/root/.cache/pip /venv/bin/pip install -r /tmp/requirements.txt \
    && mkdir -p /rootfs/data && chown 10001:10001 /rootfs/data

# ---- финальный образ: venv + код, без pip-кеша и файлов сборки ----
FROM python:3.12-slim
ENV PATH=/venv/bin:$PATH \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    DATA_DIR=/data \
    PORT=8000
# Порядок слоёв — от редко меняющихся к часто: venv, затем код.
COPY --from=build /venv /venv
COPY --from=build /rootfs/data /data
WORKDIR /srv
COPY app ./app
USER 10001:10001
EXPOSE 8000
HEALTHCHECK --interval=5s --timeout=3s --start-period=10s --retries=5 \
    CMD ["python", "-c", "import sys, urllib.request; sys.exit(urllib.request.urlopen('http://127.0.0.1:8000/health/ready', timeout=2).status != 200)"]
CMD ["python", "-m", "app"]
