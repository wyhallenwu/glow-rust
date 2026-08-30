#!/usr/bin/env bash

set -Eeuo pipefail

PROGRAM_NAME="glow"
REPOSITORY="${GLOW_REPOSITORY:-wyhallenwu/glow-rust}"
VERSION="${GLOW_VERSION:-latest}"
INSTALL_DIR="${GLOW_INSTALL_DIR:-}"
DOWNLOAD_BASE_URL="${GLOW_DOWNLOAD_BASE_URL:-}"

usage() {
    cat <<'EOF'
Install a prebuilt Glow release on macOS or Linux.

Usage:
  install.sh [options]

Options:
  --version VERSION     Install a release such as v4.0.0. Defaults to latest.
  --install-dir DIR     Install into DIR. Defaults to ~/.local/bin.
  -h, --help            Show this help and exit.

Environment:
  GLOW_VERSION             Same as --version.
  GLOW_INSTALL_DIR         Same as --install-dir.
  GLOW_REPOSITORY          GitHub owner/repository used for downloads.
  GLOW_DOWNLOAD_BASE_URL   Override the release download base URL (for testing).

The installer downloads a release archive and checks it against the release's
checksums.txt before replacing the destination binary. It never invokes sudo.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 2
}

require_value() {
    local option="$1"
    local value="${2:-}"
    [[ -n "${value}" && "${value}" != --* ]] || die "${option} requires a value"
}

while (($# > 0)); do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --version)
            require_value "$1" "${2:-}"
            VERSION="$2"
            shift 2
            ;;
        --version=*)
            VERSION="${1#*=}"
            [[ -n "${VERSION}" ]] || die "--version requires a value"
            shift
            ;;
        --install-dir)
            require_value "$1" "${2:-}"
            INSTALL_DIR="$2"
            shift 2
            ;;
        --install-dir=*)
            INSTALL_DIR="${1#*=}"
            [[ -n "${INSTALL_DIR}" ]] || die "--install-dir requires a value"
            shift
            ;;
        --)
            shift
            (($# == 0)) || die "unexpected positional argument: $1"
            ;;
        -*)
            die "unknown option: $1 (try --help)"
            ;;
        *)
            die "unexpected positional argument: $1 (try --help)"
            ;;
    esac
done

for tool in awk curl install mkdir mktemp mv tar uname; do
    command -v "${tool}" >/dev/null 2>&1 || die "required tool not found: ${tool}"
done

if [[ -z "${INSTALL_DIR}" ]]; then
    [[ -n "${HOME:-}" ]] || die "HOME is unset; pass --install-dir explicitly"
    INSTALL_DIR="${HOME}/.local/bin"
fi

OS_NAME="$(uname -s)"
MACHINE_ARCH="$(uname -m)"
case "${OS_NAME}" in
    Darwin)
        case "${MACHINE_ARCH}" in
            arm64 | aarch64) TARGET="aarch64-apple-darwin" ;;
            x86_64 | amd64) TARGET="x86_64-apple-darwin" ;;
            *) die "unsupported macOS architecture: ${MACHINE_ARCH}" ;;
        esac
        ;;
    Linux)
        case "${MACHINE_ARCH}" in
            arm64 | aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
            x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
            *) die "unsupported Linux architecture: ${MACHINE_ARCH}" ;;
        esac
        ;;
    *)
        die "unsupported operating system: ${OS_NAME}; use scripts/build.sh to build from source"
        ;;
esac

ARCHIVE_NAME="${PROGRAM_NAME}-${TARGET}.tar.gz"
if [[ "${VERSION}" == "latest" ]]; then
    RELEASE_PATH="latest/download"
else
    case "${VERSION}" in
        v*) RELEASE_TAG="${VERSION}" ;;
        *) RELEASE_TAG="v${VERSION}" ;;
    esac
    TAG_VALUE="${RELEASE_TAG#v}"
    [[ -n "${TAG_VALUE}" && "${TAG_VALUE}" != *[!0-9A-Za-z._+-]* ]] \
        || die "invalid release version: ${VERSION}"
    RELEASE_PATH="download/${RELEASE_TAG}"
fi

if [[ -n "${DOWNLOAD_BASE_URL}" ]]; then
    RELEASE_BASE_URL="${DOWNLOAD_BASE_URL%/}"
else
    RELEASE_BASE_URL="https://github.com/${REPOSITORY}/releases/${RELEASE_PATH}"
fi

INSTALL_TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/glow-install.XXXXXX")" \
    || die "cannot create a temporary directory"
DESTINATION_TEMP=""
cleanup() {
    rm -rf -- "${INSTALL_TEMP_DIR}"
    if [[ -n "${DESTINATION_TEMP}" ]]; then
        rm -f -- "${DESTINATION_TEMP}"
    fi
}
trap cleanup EXIT INT TERM

download() {
    local url="$1"
    local destination="$2"
    if [[ "${url}" == https://* ]]; then
        curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
            "${url}" --output "${destination}"
    elif [[ -n "${DOWNLOAD_BASE_URL}" ]]; then
        curl --fail --silent --show-error --location "${url}" --output "${destination}"
    else
        die "refusing a non-HTTPS download URL: ${url}"
    fi
}

sha256_file() {
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${file}" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "${file}" | awk '{print $1}'
    else
        die "SHA-256 verification requires sha256sum or shasum"
    fi
}

ARCHIVE_PATH="${INSTALL_TEMP_DIR}/${ARCHIVE_NAME}"
CHECKSUMS_PATH="${INSTALL_TEMP_DIR}/checksums.txt"
printf 'Downloading Glow %s for %s...\n' "${VERSION}" "${TARGET}"
download "${RELEASE_BASE_URL}/${ARCHIVE_NAME}" "${ARCHIVE_PATH}"
download "${RELEASE_BASE_URL}/checksums.txt" "${CHECKSUMS_PATH}"

EXPECTED_SHA256="$(
    awk -v name="${ARCHIVE_NAME}" '$2 == name || $2 == ("*" name) { print $1; exit }' \
        "${CHECKSUMS_PATH}"
)"
[[ "${#EXPECTED_SHA256}" -eq 64 && "${EXPECTED_SHA256}" != *[!0-9a-fA-F]* ]] \
    || die "checksums.txt has no valid SHA-256 entry for ${ARCHIVE_NAME}"
ACTUAL_SHA256="$(sha256_file "${ARCHIVE_PATH}")"
[[ "${ACTUAL_SHA256}" == "${EXPECTED_SHA256}" ]] \
    || die "checksum mismatch for ${ARCHIVE_NAME}"

EXTRACT_DIR="${INSTALL_TEMP_DIR}/extract"
mkdir -p "${EXTRACT_DIR}"
tar -xzf "${ARCHIVE_PATH}" -C "${EXTRACT_DIR}" "${PROGRAM_NAME}" \
    || die "cannot extract ${ARCHIVE_NAME}"
[[ -x "${EXTRACT_DIR}/${PROGRAM_NAME}" ]] \
    || die "release archive does not contain an executable ${PROGRAM_NAME}"
if ! INSTALLED_VERSION="$("${EXTRACT_DIR}/${PROGRAM_NAME}" --version 2>&1)"; then
    if [[ "${OS_NAME}" == "Linux" ]]; then
        die "the downloaded binary cannot run; Linux release binaries require glibc, so musl-based or older systems should use scripts/build.sh"
    fi
    die "the downloaded binary cannot run on this system"
fi

mkdir -p "${INSTALL_DIR}" \
    || die "cannot create ${INSTALL_DIR}; choose a writable --install-dir"
DESTINATION_TEMP="$(mktemp "${INSTALL_DIR}/.glow-install.XXXXXX")" \
    || die "cannot create a temporary file in ${INSTALL_DIR}"
install -m 0755 "${EXTRACT_DIR}/${PROGRAM_NAME}" "${DESTINATION_TEMP}" \
    || die "cannot install into ${INSTALL_DIR}; choose a writable --install-dir"
mv -f -- "${DESTINATION_TEMP}" "${INSTALL_DIR}/${PROGRAM_NAME}" \
    || die "cannot replace ${INSTALL_DIR}/${PROGRAM_NAME}"
DESTINATION_TEMP=""
printf 'Installed %s to %s\n' "${INSTALLED_VERSION}" "${INSTALL_DIR}/${PROGRAM_NAME}"

case ":${PATH:-}:" in
    *:"${INSTALL_DIR}":*) ;;
    *)
        printf 'Add %s to PATH, for example:\n  export PATH="%s:$PATH"\n' \
            "${INSTALL_DIR}" "${INSTALL_DIR}"
        ;;
esac
