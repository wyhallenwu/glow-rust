#!/usr/bin/env bash

set -Eeuo pipefail

PROGRAM_NAME="glow"
CALLER_DIR="$(pwd -P)"
SCRIPT_LOCATION="${BASH_SOURCE[0]}"
if [[ "${SCRIPT_LOCATION}" == */* ]]; then
    SCRIPT_PARENT="${SCRIPT_LOCATION%/*}"
else
    SCRIPT_PARENT="."
fi
SCRIPT_DIR="$(CDPATH= cd -- "${SCRIPT_PARENT}" && pwd -P)"
REPO_ROOT="$(CDPATH= cd -- "${SCRIPT_DIR}/.." && pwd -P)"
ENV_PREFIX="${PREFIX:-}"

INSTALL=true
INSTALL_DIR=""
PREFIX=""
TARGET=""
INSTALL_DIR_SET=false
PREFIX_SET=false

usage() {
    cat <<'EOF'
Build Glow from source on macOS or Linux.

Usage:
  scripts/build.sh [options]

Options:
  --no-install          Build only; do not copy the binary into a bin directory.
  --install-dir DIR     Install directly into DIR (for example, ~/.local/bin).
  --prefix PREFIX       Install into PREFIX/bin. Defaults to $PREFIX or ~/.local.
  --target TRIPLE       Build for a Rust target triple (for example,
                        aarch64-unknown-linux-gnu).
  -h, --help            Show this help and exit.

The script builds with `cargo build --locked --release`. If Rust is missing, it
installs the stable minimal toolchain through rustup non-interactively. It never
invokes sudo. Cross targets may also require a platform linker/toolchain supplied
by your operating system.
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
        --no-install)
            INSTALL=false
            shift
            ;;
        --install-dir)
            require_value "$1" "${2:-}"
            INSTALL_DIR="$2"
            INSTALL_DIR_SET=true
            shift 2
            ;;
        --install-dir=*)
            INSTALL_DIR="${1#*=}"
            [[ -n "${INSTALL_DIR}" ]] || die "--install-dir requires a value"
            INSTALL_DIR_SET=true
            shift
            ;;
        --prefix)
            require_value "$1" "${2:-}"
            PREFIX="$2"
            PREFIX_SET=true
            shift 2
            ;;
        --prefix=*)
            PREFIX="${1#*=}"
            [[ -n "${PREFIX}" ]] || die "--prefix requires a value"
            PREFIX_SET=true
            shift
            ;;
        --target)
            require_value "$1" "${2:-}"
            TARGET="$2"
            shift 2
            ;;
        --target=*)
            TARGET="${1#*=}"
            [[ -n "${TARGET}" ]] || die "--target requires a value"
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

if ! ${INSTALL} && (${INSTALL_DIR_SET} || ${PREFIX_SET}); then
    die "--no-install cannot be combined with --install-dir or --prefix"
fi
if ${INSTALL_DIR_SET} && ${PREFIX_SET}; then
    die "--install-dir and --prefix are mutually exclusive"
fi

command -v uname >/dev/null 2>&1 || die "required tool not found: uname"
if ${INSTALL}; then
    for tool in mkdir install; do
        command -v "${tool}" >/dev/null 2>&1 || die "required install tool not found: ${tool}"
    done
fi

OS_NAME="$(uname -s)"
MACHINE_ARCH="$(uname -m)"
case "${OS_NAME}" in
    Darwin)
        PLATFORM="macOS"
        case "${MACHINE_ARCH}" in
            arm64 | aarch64) DETECTED_TARGET="aarch64-apple-darwin" ;;
            x86_64 | amd64) DETECTED_TARGET="x86_64-apple-darwin" ;;
            *) die "unsupported macOS architecture: ${MACHINE_ARCH}" ;;
        esac
        ;;
    Linux)
        PLATFORM="Linux"
        case "${MACHINE_ARCH}" in
            x86_64 | amd64) DETECTED_TARGET="x86_64-unknown-linux-gnu" ;;
            arm64 | aarch64) DETECTED_TARGET="aarch64-unknown-linux-gnu" ;;
            armv7l | armv7) DETECTED_TARGET="armv7-unknown-linux-gnueabihf" ;;
            ppc64le) DETECTED_TARGET="powerpc64le-unknown-linux-gnu" ;;
            riscv64) DETECTED_TARGET="riscv64gc-unknown-linux-gnu" ;;
            s390x) DETECTED_TARGET="s390x-unknown-linux-gnu" ;;
            *) die "unsupported Linux architecture: ${MACHINE_ARCH}" ;;
        esac
        ;;
    *)
        die "unsupported operating system: ${OS_NAME}; this script supports macOS and Linux"
        ;;
esac

printf 'Detected %s on %s (Rust target hint: %s).\n' \
    "${PLATFORM}" "${MACHINE_ARCH}" "${DETECTED_TARGET}"

if [[ ! -f "${REPO_ROOT}/Cargo.toml" || ! -f "${REPO_ROOT}/Cargo.lock" ]]; then
    die "Cargo.toml or Cargo.lock is missing from ${REPO_ROOT}"
fi

manifest_rust_version() {
    local key
    local value
    while IFS='=' read -r key value; do
        key="${key//[[:space:]]/}"
        if [[ "${key}" == "rust-version" ]]; then
            value="${value%%#*}"
            value="${value//[[:space:]]/}"
            value="${value//\"/}"
            value="${value//\'/}"
            printf '%s\n' "${value}"
            return 0
        fi
    done < "${REPO_ROOT}/Cargo.toml"
    return 1
}

version_at_least() {
    local actual="$1"
    local required="$2"
    local actual_major actual_minor actual_patch
    local required_major required_minor required_patch
    IFS='.' read -r actual_major actual_minor actual_patch <<< "${actual}"
    IFS='.' read -r required_major required_minor required_patch <<< "${required}"
    actual_patch="${actual_patch%%-*}"
    required_patch="${required_patch%%-*}"
    actual_patch="${actual_patch:-0}"
    required_patch="${required_patch:-0}"

    for component in \
        "${actual_major}" "${actual_minor}" "${actual_patch}" \
        "${required_major}" "${required_minor}" "${required_patch}"; do
        [[ -n "${component}" && "${component}" != *[!0-9]* ]] || return 1
    done

    if ((10#${actual_major} != 10#${required_major})); then
        ((10#${actual_major} > 10#${required_major}))
    elif ((10#${actual_minor} != 10#${required_minor})); then
        ((10#${actual_minor} > 10#${required_minor}))
    else
        ((10#${actual_patch} >= 10#${required_patch}))
    fi
}

MIN_RUST_VERSION="$(manifest_rust_version || true)"
[[ -n "${MIN_RUST_VERSION}" ]] \
    || die "Cargo.toml does not declare package.rust-version"

load_cargo_environment() {
    local cargo_home="${CARGO_HOME:-}"
    if [[ -z "${cargo_home}" ]]; then
        [[ -n "${HOME:-}" ]] || die "HOME is unset; set CARGO_HOME before installing Rust"
        cargo_home="${HOME}/.cargo"
    fi
    if [[ -f "${cargo_home}/env" ]]; then
        # This file is generated by rustup and updates PATH for this process.
        # shellcheck disable=SC1090
        source "${cargo_home}/env"
    else
        export PATH="${cargo_home}/bin:${PATH}"
    fi
}

install_rust_toolchain() {
    printf '%s\n' \
        'Rust was not found. Installing the stable minimal toolchain with rustup.' \
        'This step is non-interactive (-y), does not use sudo, and does not edit PATH files.' >&2

    if command -v rustup >/dev/null 2>&1; then
        rustup toolchain install stable --profile minimal
        rustup default stable
    elif command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
            https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
    elif command -v wget >/dev/null 2>&1; then
        wget --https-only --secure-protocol=TLSv1_2 -qO- https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
    else
        die "Rust is missing and neither curl nor wget is available to install rustup"
    fi
    load_cargo_environment
}

if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    install_rust_toolchain
fi
command -v cargo >/dev/null 2>&1 || die "cargo is unavailable after rustup installation"
command -v rustc >/dev/null 2>&1 || die "rustc is unavailable after rustup installation"

CARGO_COMMAND=(cargo)
RUSTC_COMMAND=(rustc)
BUILD_TOOLCHAIN=""
RUST_VERSION_OUTPUT="$(rustc --version)"
read -r _ ACTIVE_RUST_VERSION _ <<< "${RUST_VERSION_OUTPUT}"
if ! version_at_least "${ACTIVE_RUST_VERSION}" "${MIN_RUST_VERSION}"; then
    if ! command -v rustup >/dev/null 2>&1; then
        die "Rust ${ACTIVE_RUST_VERSION} is too old; Glow requires ${MIN_RUST_VERSION} or newer. Install a current stable toolchain with rustup and retry"
    fi
    printf 'Rust %s is older than the required %s; installing/updating stable with rustup for this build.\n' \
        "${ACTIVE_RUST_VERSION}" "${MIN_RUST_VERSION}" >&2
    rustup toolchain install stable --profile minimal
    CARGO_COMMAND=(rustup run stable cargo)
    RUSTC_COMMAND=(rustup run stable rustc)
    BUILD_TOOLCHAIN="stable"
    RUST_VERSION_OUTPUT="$("${RUSTC_COMMAND[@]}" --version)"
    read -r _ ACTIVE_RUST_VERSION _ <<< "${RUST_VERSION_OUTPUT}"
    version_at_least "${ACTIVE_RUST_VERSION}" "${MIN_RUST_VERSION}" \
        || die "stable Rust ${ACTIVE_RUST_VERSION} still does not meet required version ${MIN_RUST_VERSION}"
fi
printf 'Using Rust %s (minimum required: %s).\n' \
    "${ACTIVE_RUST_VERSION}" "${MIN_RUST_VERSION}"

RUST_HOST=""
while IFS= read -r rust_version_line; do
    case "${rust_version_line}" in
        host:\ *) RUST_HOST="${rust_version_line#host: }" ;;
    esac
done < <("${RUSTC_COMMAND[@]}" -vV)
[[ -n "${RUST_HOST}" ]] || die "could not determine the active Rust host target"
printf 'Active Rust host: %s.\n' "${RUST_HOST}"

if [[ -n "${TARGET}" ]] && command -v rustup >/dev/null 2>&1; then
    target_installed=false
    rustup_list_command=(rustup target list --installed)
    if [[ -n "${BUILD_TOOLCHAIN}" ]]; then
        rustup_list_command+=(--toolchain "${BUILD_TOOLCHAIN}")
    fi
    while IFS= read -r installed_target; do
        if [[ "${installed_target}" == "${TARGET}" ]]; then
            target_installed=true
            break
        fi
    done < <("${rustup_list_command[@]}")
    if ! ${target_installed}; then
        printf 'Installing Rust standard library for target %s.\n' "${TARGET}"
        rustup_add_command=(rustup target add)
        if [[ -n "${BUILD_TOOLCHAIN}" ]]; then
            rustup_add_command+=(--toolchain "${BUILD_TOOLCHAIN}")
        fi
        rustup_add_command+=("${TARGET}")
        "${rustup_add_command[@]}"
    fi
fi

CC_READY=false
if [[ -n "${CC:-}" ]]; then
    CC_READY=true
elif command -v cc >/dev/null 2>&1 && cc --version >/dev/null 2>&1; then
    CC_READY=true
fi
if ! ${CC_READY}; then
    if [[ "${PLATFORM}" == "macOS" ]]; then
        CC_HINT='Install the Command Line Tools by running: xcode-select --install'
    elif command -v apt-get >/dev/null 2>&1; then
        CC_HINT='Install the compiler toolchain with your package manager: apt-get install build-essential'
    elif command -v dnf >/dev/null 2>&1; then
        CC_HINT='Install the compiler toolchain with your package manager: dnf install gcc gcc-c++ make'
    elif command -v apk >/dev/null 2>&1; then
        CC_HINT='Install the compiler toolchain with your package manager: apk add build-base'
    elif command -v pacman >/dev/null 2>&1; then
        CC_HINT='Install the compiler toolchain with your package manager: pacman -S base-devel'
    elif command -v zypper >/dev/null 2>&1; then
        CC_HINT='Install the compiler toolchain with your package manager: zypper install -t pattern devel_basis'
    else
        CC_HINT='Install a C compiler and linker (the `cc` command) using your Linux distribution package manager'
    fi
    if [[ -z "${TARGET}" || "${TARGET}" == "${RUST_HOST}" ]]; then
        die "a working C compiler/linker is required. ${CC_HINT}"
    fi
    printf 'warning: no host `cc` was found. %s. A target-specific linker may also be required.\n' \
        "${CC_HINT}" >&2
fi

BUILD_TARGET="${TARGET:-${RUST_HOST}}"
CARGO_METADATA="$({
    cd -- "${REPO_ROOT}"
    "${CARGO_COMMAND[@]}" metadata --locked --no-deps --format-version 1
})"
METADATA_TARGET_SUFFIX="${CARGO_METADATA#*\"target_directory\":\"}"
[[ "${METADATA_TARGET_SUFFIX}" != "${CARGO_METADATA}" ]] \
    || die "cargo metadata did not report a target_directory"
CARGO_BUILD_DIR="${METADATA_TARGET_SUFFIX%%\"*}"
[[ -n "${CARGO_BUILD_DIR}" ]] || die "cargo metadata returned an empty target_directory"

printf 'Building %s for %s with the locked dependency graph...\n' \
    "${PROGRAM_NAME}" "${BUILD_TARGET}"

build_command=("${CARGO_COMMAND[@]}" build --locked --release)
if [[ -n "${TARGET}" ]]; then
    build_command+=(--target "${TARGET}")
fi
(
    cd -- "${REPO_ROOT}"
    "${build_command[@]}"
)

if [[ -n "${TARGET}" ]]; then
    BINARY_PATH="${CARGO_BUILD_DIR}/${TARGET}/release/${PROGRAM_NAME}"
else
    BINARY_PATH="${CARGO_BUILD_DIR}/release/${PROGRAM_NAME}"
fi
[[ -f "${BINARY_PATH}" ]] || die "build completed but binary was not found: ${BINARY_PATH}"

if ! ${INSTALL}; then
    printf 'Build complete: %s\n' "${BINARY_PATH}"
    exit 0
fi

absolute_from_caller() {
    local path="$1"
    if [[ "${path}" == /* ]]; then
        printf '%s\n' "${path}"
    else
        printf '%s/%s\n' "${CALLER_DIR}" "${path}"
    fi
}

if ${INSTALL_DIR_SET}; then
    RESOLVED_INSTALL_DIR="$(absolute_from_caller "${INSTALL_DIR}")"
else
    if ! ${PREFIX_SET}; then
        if [[ -n "${ENV_PREFIX}" ]]; then
            PREFIX="${ENV_PREFIX}"
        elif [[ -n "${HOME:-}" ]]; then
            PREFIX="${HOME}/.local"
        else
            PREFIX="${CALLER_DIR}/.local"
        fi
    fi
    RESOLVED_INSTALL_DIR="$(absolute_from_caller "${PREFIX}/bin")"
fi

if [[ -n "${TARGET}" && "${TARGET}" != "${RUST_HOST}" ]]; then
    printf 'warning: installing a cross-built %s binary on host %s.\n' \
        "${TARGET}" "${RUST_HOST}" >&2
fi

mkdir -p "${RESOLVED_INSTALL_DIR}" \
    || die "cannot create ${RESOLVED_INSTALL_DIR}; choose a writable --install-dir (sudo is not used)"
install -m 0755 "${BINARY_PATH}" "${RESOLVED_INSTALL_DIR}/${PROGRAM_NAME}" \
    || die "cannot install into ${RESOLVED_INSTALL_DIR}; choose a writable directory (sudo is not used)"

printf 'Installed %s\n' "${RESOLVED_INSTALL_DIR}/${PROGRAM_NAME}"
