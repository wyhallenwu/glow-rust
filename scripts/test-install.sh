#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "${BASH_SOURCE[0]%/*}" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "${SCRIPT_DIR}/.." && pwd -P)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/glow-install-test.XXXXXX")"
cleanup() {
    rm -rf -- "${TEST_ROOT}"
}
trap cleanup EXIT INT TERM

case "$(uname -s):$(uname -m)" in
    Darwin:arm64 | Darwin:aarch64) TARGET="aarch64-apple-darwin" ;;
    Darwin:x86_64 | Darwin:amd64) TARGET="x86_64-apple-darwin" ;;
    Linux:arm64 | Linux:aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    Linux:x86_64 | Linux:amd64) TARGET="x86_64-unknown-linux-gnu" ;;
    *)
        printf 'Skipping installer test on an unsupported platform.\n'
        exit 0
        ;;
esac

RELEASE_DIR="${TEST_ROOT}/release"
PAYLOAD_DIR="${TEST_ROOT}/payload"
INSTALL_DIR="${TEST_ROOT}/install dir"
ARCHIVE_NAME="glow-${TARGET}.tar.gz"
mkdir -p "${RELEASE_DIR}" "${PAYLOAD_DIR}"
printf '%s\n' '#!/usr/bin/env bash' 'printf '\''glow 99.0.0-test\n'\''' \
    > "${PAYLOAD_DIR}/glow"
chmod 0755 "${PAYLOAD_DIR}/glow"
tar -czf "${RELEASE_DIR}/${ARCHIVE_NAME}" -C "${PAYLOAD_DIR}" glow

if command -v sha256sum >/dev/null 2>&1; then
    (
        cd -- "${RELEASE_DIR}"
        sha256sum "${ARCHIVE_NAME}" > checksums.txt
    )
else
    (
        cd -- "${RELEASE_DIR}"
        shasum -a 256 "${ARCHIVE_NAME}" > checksums.txt
    )
fi

GLOW_DOWNLOAD_BASE_URL="file://${RELEASE_DIR}" \
    bash "${REPO_ROOT}/install.sh" --install-dir "${INSTALL_DIR}"
[[ -x "${INSTALL_DIR}/glow" ]] || {
    printf 'installer did not create an executable binary\n' >&2
    exit 1
}
[[ "$("${INSTALL_DIR}/glow" --version)" == "glow 99.0.0-test" ]] || {
    printf 'installed binary returned an unexpected version\n' >&2
    exit 1
}

printf '%s\n' '#!/usr/bin/env bash' 'exit 1' > "${PAYLOAD_DIR}/glow"
chmod 0755 "${PAYLOAD_DIR}/glow"
tar -czf "${RELEASE_DIR}/${ARCHIVE_NAME}" -C "${PAYLOAD_DIR}" glow
if command -v sha256sum >/dev/null 2>&1; then
    (
        cd -- "${RELEASE_DIR}"
        sha256sum "${ARCHIVE_NAME}" > checksums.txt
    )
else
    (
        cd -- "${RELEASE_DIR}"
        shasum -a 256 "${ARCHIVE_NAME}" > checksums.txt
    )
fi
if GLOW_DOWNLOAD_BASE_URL="file://${RELEASE_DIR}" \
    bash "${REPO_ROOT}/install.sh" --install-dir "${INSTALL_DIR}" \
    > "${TEST_ROOT}/incompatible.log" 2>&1; then
    printf 'installer accepted a binary that failed its version check\n' >&2
    exit 1
fi
[[ "$("${INSTALL_DIR}/glow" --version)" == "glow 99.0.0-test" ]] || {
    printf 'installer replaced the existing binary before validating the new one\n' >&2
    exit 1
}

printf 'corruption' >> "${RELEASE_DIR}/${ARCHIVE_NAME}"
if GLOW_DOWNLOAD_BASE_URL="file://${RELEASE_DIR}" \
    bash "${REPO_ROOT}/install.sh" --install-dir "${TEST_ROOT}/bad-bin" \
    > "${TEST_ROOT}/failure.log" 2>&1; then
    printf 'installer accepted an archive with an invalid checksum\n' >&2
    exit 1
fi
grep -q 'checksum mismatch' "${TEST_ROOT}/failure.log" || {
    printf 'installer did not report the checksum mismatch\n' >&2
    exit 1
}

printf 'Installer tests passed.\n'
