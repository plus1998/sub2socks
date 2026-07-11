FROM node:22-bookworm-slim AS frontend-builder

WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend ./
RUN npm run build

FROM rust:1.75-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates libssl3 pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src ./src
COPY --from=frontend-builder /src/frontend/dist/index.html ./src/static/index.html
COPY --from=frontend-builder /src/frontend/dist/assets/ ./src/static/

RUN cargo build --release --locked \
    && install -Dm755 target/release/rust-proxy-manager /out/rust-proxy-manager

FROM debian:bookworm-slim AS runtime

# Mihomo is bundled in the repo (linux/amd64 v1.19.27).
# Override with a different binary via docker-compose volume mount if needed.
COPY mihomo /usr/local/bin/mihomo

RUN chmod 0755 /usr/local/bin/mihomo \
    && apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /app --shell /usr/sbin/nologin app \
    && mkdir -p /data \
    && chown -R app:app /app /data

WORKDIR /app
COPY --from=builder /out/rust-proxy-manager /app/rust-proxy-manager

ENV PORT=3000
ENV RUST_PROXY_MANAGER_DB=/data/proxy_manager.db
ENV RUST_PROXY_MANAGER_DATA_DIR=/data
ENV MIHOMO_BINARY=/usr/local/bin/mihomo

VOLUME ["/data"]
EXPOSE 3000 9999

USER app
ENTRYPOINT ["/app/rust-proxy-manager"]
