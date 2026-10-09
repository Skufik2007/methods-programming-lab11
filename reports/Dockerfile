# syntax=docker/dockerfile:1

# Базовые образы закреплены по digest: тег со временем указывает на другой образ,
# а digest — всегда на один и тот же, поэтому сборка и кеш слоёв воспроизводимы.
# Обновить: docker buildx imagetools inspect <образ:тег> и заменить sha256.

ARG PYTHON_IMAGE=python:3.12-slim@sha256:a6e34c598f2467ed0e9a8d349809fcd8b5c603269512df273a0bb1784edc11b1

# ---- сборка: зависимости в отдельном venv ----
FROM ${PYTHON_IMAGE} AS build
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
RUN python -m venv /venv
# Слой зависимостей зависит только от requirements.txt; pip-кеш — в cache mount
# (повторная установка после смены одной версии не качает всё заново).
COPY requirements.txt /tmp/requirements.txt
RUN --mount=type=cache,target=/root/.cache/pip /venv/bin/pip install -r /tmp/requirements.txt \
    && mkdir -p /rootfs/data

# ---- финальный образ: venv + код, без pip-кеша и файлов сборки ----
FROM ${PYTHON_IMAGE}
ENV PATH=/venv/bin:$PATH \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    DATA_DIR=/data \
    PORT=8000
# Порядок слоёв — от редко меняющихся к часто: venv, затем код.
COPY --from=build /venv /venv
# --chown обязателен: COPY --from не переносит владельца, без него /data
# принадлежал бы root и сервис под 10001 не смог бы писать в volume.
COPY --from=build --chown=10001:10001 /rootfs/data /data
WORKDIR /srv
COPY app ./app
USER 10001:10001
EXPOSE 8000
HEALTHCHECK --interval=5s --timeout=3s --start-period=10s --retries=5 \
    CMD ["python", "-c", "import sys, urllib.request; sys.exit(urllib.request.urlopen('http://127.0.0.1:8000/health/ready', timeout=2).status != 200)"]
CMD ["python", "-m", "app"]
