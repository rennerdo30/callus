# Multi-stage production Dockerfile for Callus (zero GPU, minimal Linux runtime).
# Targets LiteVM deployments (<= 200 MB total stack RAM with PostgreSQL).

FROM rust:1.99.1-slim-bookworm AS builder

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        gcc \
        libc6-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates

RUN cargo build --release --locked --bin callus \
    && strip /build/target/release/callus

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home callus

COPY --from=builder /build/target/release/callus /usr/local/bin/callus

USER callus
ENV CALLUS_BIND=0.0.0.0:8080
EXPOSE 8080

HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/callus"]
CMD ["serve"]
