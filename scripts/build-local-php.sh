#!/usr/bin/env bash

set -euo pipefail

readonly POX_RUNTIME_SOURCE="${POX_RUNTIME_SOURCE:-../pox-runtime}"
readonly PHP_CONFIG="${PHP_CONFIG:-${XDG_BIN_HOME:-${HOME}/.local/bin}/pox-php-config}"
readonly PHP_VERSION="${POX_PHP_VERSION:-8.5}"

if [[ ! -f "${POX_RUNTIME_SOURCE}/Makefile" ]]; then
    echo "Missing sibling pox-runtime checkout at ${POX_RUNTIME_SOURCE}" >&2
    echo "Clone it with: git clone https://github.com/shyim/pox-runtime ${POX_RUNTIME_SOURCE}" >&2
    exit 1
fi

case "$(uname -m)" in
    x86_64) runtime_arch="x86_64" ;;
    aarch64|arm64) runtime_arch="aarch64" ;;
    *) echo "Unsupported runtime architecture: $(uname -m)" >&2; exit 1 ;;
esac

case "$(uname -s)" in
    Darwin)
        readonly TARGET="${runtime_arch}-apple-darwin"
        readonly LIBRARY_NAME="libpox_php.dylib"
        runtime_libc=""
        ;;
    Linux)
        if ldd --version 2>&1 | grep -qi musl; then
            runtime_libc="musl"
        else
            runtime_libc="gnu"
        fi
        readonly TARGET="${runtime_arch}-unknown-linux-${runtime_libc}"
        readonly LIBRARY_NAME="libpox_php.so"
        ;;
    *) echo "Unsupported runtime host: $(uname -s)" >&2; exit 1 ;;
esac

if [[ -x "${PHP_CONFIG}" ]]; then
    POX_ALLOW_DYNAMIC_CXX=1 make -C "${POX_RUNTIME_SOURCE}" test \
        PHP_CONFIG="${PHP_CONFIG}" \
        TARGET="${TARGET}" \
        RUNTIME_REVISION=dev
else
    (
        cd "${POX_RUNTIME_SOURCE}"
        PHP_VERSION="${PHP_VERSION}" \
        TARGET="${TARGET}" \
        RUNTIME_REVISION=dev \
            ./scripts/build-php-runtime.sh
    )
fi

echo "Runtime ready at ${POX_RUNTIME_SOURCE}/build/${LIBRARY_NAME}"
