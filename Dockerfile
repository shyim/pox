# syntax=docker/dockerfile:1
ARG BUILDER_IMAGE=ghcr.io/shyim/pox-builder:latest

FROM ${BUILDER_IMAGE} AS builder
WORKDIR /work
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /work/target/release/pox /usr/local/bin/pox
ENTRYPOINT ["/usr/local/bin/pox"]
