#!/usr/bin/env bash
set -euo pipefail

readonly REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPOSITORY_ROOT}"

: "${VERSION:?VERSION is required}"
: "${TARGET:?TARGET is required}"

if [[ ! "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?([+][0-9A-Za-z.-]+)?$ ]]; then
    echo "Invalid release version: ${VERSION}" >&2
    exit 1
fi
if [[ ! "${TARGET}" =~ ^[0-9A-Za-z_.-]+$ ]]; then
    echo "Invalid release target: ${TARGET}" >&2
    exit 1
fi

readonly OUTPUT_DIR="${OUTPUT_DIR:-dist}"
readonly POX_BINARY="${POX_BINARY:-${OUTPUT_DIR}/pox}"
readonly PACKAGE_NAME="pox-${VERSION}-${TARGET}"
readonly ARCHIVE_NAME="${PACKAGE_NAME}.tar.gz"

if [[ ! -x "${POX_BINARY}" ]]; then
    echo "Pox binary is missing or not executable: ${POX_BINARY}" >&2
    exit 1
fi

"${POX_BINARY}" --help >/dev/null
mkdir -p "${OUTPUT_DIR}"

staging_root="$(mktemp -d "${TMPDIR:-/tmp}/pox-release.XXXXXX")"
cleanup() {
    rm -rf "${staging_root}"
}
trap cleanup EXIT

mkdir -p "${staging_root}/${PACKAGE_NAME}"
install -m 0755 "${POX_BINARY}" "${staging_root}/${PACKAGE_NAME}/pox"
install -m 0644 README.md LICENSE "${staging_root}/${PACKAGE_NAME}/"

tar -C "${staging_root}" -czf "${OUTPUT_DIR}/${ARCHIVE_NAME}" "${PACKAGE_NAME}"

(
    cd "${OUTPUT_DIR}"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${ARCHIVE_NAME}" > "${ARCHIVE_NAME}.sha256"
    else
        shasum -a 256 "${ARCHIVE_NAME}" > "${ARCHIVE_NAME}.sha256"
    fi
)

echo "Packaged ${OUTPUT_DIR}/${ARCHIVE_NAME}"
