# syntax=docker/dockerfile:1

# ---- build stage -----------------------------------------------------------
FROM rust:1-slim-bookworm AS builder

# `ring` (via rustls) needs a C toolchain; the slim image does not ship one.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Compile dependencies first so they are cached independently of the sources.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

# ---- runtime stage ---------------------------------------------------------
# Distroless: no shell, no package manager, just glibc/libgcc and the binary.
FROM gcr.io/distroless/cc-debian12:nonroot

LABEL org.opencontainers.image.title="tailcompany-wp-proxy" \
      org.opencontainers.image.description="Caching proxy for the Tail Company WooCommerce Store API" \
      org.opencontainers.image.source="https://github.com/OpenTails/tail_app_services" \
      org.opencontainers.image.url="https://github.com/OpenTails/tail_app_services" \
      org.opencontainers.image.licenses="GPL-3.0-only" \
      org.opencontainers.image.vendor="OpenTails"

COPY --from=builder /app/target/release/tailcompany-wp-proxy /usr/local/bin/tailcompany-wp-proxy

EXPOSE 3000
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/tailcompany-wp-proxy"]
