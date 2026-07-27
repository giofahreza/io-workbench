FROM rust:1-slim-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps

RUN cargo build --release -p iowb-cli --bin io-workbench

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git openssh-client bash \
    && rm -rf /var/lib/apt/lists/*

ENV IO_WORKBENCH_HOST=0.0.0.0 \
    IO_WORKBENCH_PORT=8787 \
    IO_WORKBENCH_CONFIG_DIR=/data \
    IO_WORKBENCH_WORKSPACE_ROOT=/workspace

RUN mkdir -p /data /workspace

COPY --from=builder /app/target/release/io-workbench /usr/local/bin/io-workbench

EXPOSE 8787
VOLUME ["/data", "/workspace"]
ENTRYPOINT ["io-workbench"]
CMD ["start"]
