# syntax=docker/dockerfile:1

# ---- сборка: Alpine = musl, Rust по умолчанию линкует musl-бинарь статически ----
FROM rust:1.99-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src

# Слой 1 — только зависимости. Собираем проект с пустым main.rs: слой зависит
# лишь от Cargo.toml/Cargo.lock и при правке кода берётся из кеша целиком
# (сборка ~40 крейтов не повторяется). Реестр крейтов — в cache mount,
# чтобы не скачивать его заново даже при смене зависимостей.
COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/processor target/release/deps/processor-* target/release/.fingerprint/processor-*

# Слой 2 — код. Пересобирается только сам processor.
# touch: COPY сохраняет время изменения из контекста, и оно может оказаться старше
# заглушки — тогда cargo решил бы, что пересобирать нечего, и оставил пустой main.
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    touch src/main.rs \
    && cargo build --release --locked \
    && cp target/release/processor /processor

# В scratch нет mkdir/chown — готовим каталог volume здесь. Docker при первом
# подключении именованного volume копирует в него содержимое и владельца /data
# из образа, поэтому непривилегированный пользователь сможет туда писать.
RUN mkdir -p /rootfs/data && chown 10001:10001 /rootfs/data

# ---- финальный образ: пустой, только статический бинарь ----
FROM scratch
COPY --from=build /processor /processor
# --chown обязателен: COPY --from не переносит владельца, без него /data
# принадлежал бы root и сервис под 10001 не смог бы писать в volume.
COPY --from=build --chown=10001:10001 /rootfs/data /data
USER 10001:10001
ENV DATA_DIR=/data \
    HEALTH_ADDR=0.0.0.0:9090
EXPOSE 9090
HEALTHCHECK --interval=5s --timeout=3s --start-period=5s --retries=5 CMD ["/processor", "healthcheck"]
# exec-форма: processor — PID 1 и сам получает SIGTERM от docker stop.
ENTRYPOINT ["/processor"]
