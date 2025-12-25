# syntax=docker/dockerfile:1
#checkov:skip=CKV_DOCKER_2
#checkov:skip=CKV_DOCKER_3
ARG BUILDER_IMAGE=ghcr.io/shyim/phpx-builder:latest

FROM ${BUILDER_IMAGE} AS builder

WORKDIR /work
COPY . .

RUN cargo build --release

FROM scratch

COPY --from=builder /work/target/release/phpx /phpx
