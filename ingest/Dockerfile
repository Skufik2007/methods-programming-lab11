# syntax=docker/dockerfile:1

# Базовые образы закреплены по digest: тег со временем указывает на другой образ,
# а digest — всегда на один и тот же, поэтому сборка и кеш слоёв воспроизводимы.
# Обновить: docker buildx imagetools inspect <образ:тег> и заменить sha256.

# ---- сборка: статический бинарь без cgo ----
FROM golang:1.27-alpine@sha256:738d1cf061836894ff6bb8c33881080ac66de8cf0586615012a0c8f592649cfa AS build
WORKDIR /src

# Слой зависимостей: меняется только вместе с go.mod/go.sum.
COPY go.mod go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod go mod download

# Код. Кеш компиляции Go — в cache mount: пересобираются только изменённые пакеты.
COPY *.go ./
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /ingest . \
    && mkdir -p /rootfs/data

# ---- финальный образ: scratch, только бинарь ----
FROM scratch
COPY --from=build /ingest /ingest
# --chown обязателен: COPY --from не переносит владельца, без него /data
# принадлежал бы root и сервис под 10001 не смог бы писать в volume.
COPY --from=build --chown=10001:10001 /rootfs/data /data
USER 10001:10001
ENV ADDR=:8080 \
    DATA_DIR=/data
EXPOSE 8080
HEALTHCHECK --interval=5s --timeout=3s --start-period=5s --retries=5 CMD ["/ingest", "-healthcheck"]
ENTRYPOINT ["/ingest"]
