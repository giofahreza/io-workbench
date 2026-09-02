#!/bin/sh
# Install io-workbench from a GitHub Release into the current user's home directory.
#
# This script intentionally never uses sudo. It downloads the matching Linux or
# macOS archive, verifies its SHA-256 checksum, and installs the two CLI names
# into a user-owned bin directory. Starting a server remains an explicit choice.

set -eu

REPOSITORY="${IO_WORKBENCH_REPOSITORY:-giofahreza/io-workbench}"
VERSION="${IO_WORKBENCH_VERSION:-latest}"
TMP_DIR=""

usage() {
    printf '%s\n' \
        'Usage: install.sh [--version <tag>]' \
        '' \
        'Installs the matching io-workbench GitHub Release for Linux or macOS.' \
        '' \
        'Options:' \
        '  --version <tag>  Install a release such as v0.1.0 (or 0.1.0).' \
        '  --help           Show this help.' \
        '' \
        'Environment overrides:' \
        '  IO_WORKBENCH_VERSION      Same as --version.' \
        '  IO_WORKBENCH_BIN_DIR      Destination for io-workbench and iowb.' \
        '  IO_WORKBENCH_REPOSITORY   GitHub owner/repository (advanced use).'
}

note() {
    printf '%s\n' "io-workbench installer: $*"
}

warn() {
    printf '%s\n' "io-workbench installer: warning: $*" >&2
}

die() {
    printf '%s\n' "io-workbench installer: error: $*" >&2
    exit 1
}

cleanup() {
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}

trap 'cleanup' 0
trap 'cleanup; exit 1' 1 2 3 15

download() {
    url=$1
    destination=$2

    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --show-error --retry 3 --connect-timeout 15 \
            --output "$destination" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --tries=3 --timeout=30 --output-document="$destination" "$url"
    else
        die 'curl or wget is required to download a release.'
    fi
}

sha256_file() {
    file=$1

    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$file" | awk '{print $NF}'
    else
        die 'sha256sum, shasum, or openssl is required to verify the release.'
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || die '--version needs a release tag.'
            VERSION=$2
            shift 2
            ;;
        --version=*)
            VERSION=$(printf '%s' "$1" | sed 's/^--version=//')
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1 (run with --help for usage)"
            ;;
    esac
done

[ -n "$(printenv HOME 2>/dev/null || true)" ] \
    || die 'HOME is not set; choose a user account before running the installer.'

case "$REPOSITORY" in
    */*) ;;
    *) die 'IO_WORKBENCH_REPOSITORY must be in owner/repository form.' ;;
esac
case "$REPOSITORY" in
    *[!A-Za-z0-9._/-]*|*//*|/*|*/)
        die 'IO_WORKBENCH_REPOSITORY contains unsupported characters.'
        ;;
esac

OS_NAME=$(uname -s 2>/dev/null || true)
MACHINE=$(uname -m 2>/dev/null || true)
case "$OS_NAME" in
    Linux)
        case "$MACHINE" in
            x86_64|amd64) TARGET=linux-x86_64 ;;
            aarch64|arm64) TARGET=linux-aarch64 ;;
            *) die "unsupported Linux CPU architecture: ${MACHINE:-unknown}." ;;
        esac
        ;;
    Darwin)
        # A shell running under Rosetta reports x86_64. Prefer the native
        # Apple Silicon archive when macOS identifies the translated process.
        if [ "$MACHINE" = x86_64 ] \
            && command -v sysctl >/dev/null 2>&1 \
            && [ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
            MACHINE=arm64
        fi
        case "$MACHINE" in
            x86_64|amd64) TARGET=macos-x86_64 ;;
            aarch64|arm64) TARGET=macos-aarch64 ;;
            *) die "unsupported macOS CPU architecture: ${MACHINE:-unknown}." ;;
        esac
        ;;
    *)
        die "unsupported operating system: ${OS_NAME:-unknown}. Use install.ps1 on Windows."
        ;;
esac

TMP_BASE=$(printenv TMPDIR 2>/dev/null || printf '%s' /tmp)
TMP_DIR=$(mktemp -d "$TMP_BASE/io-workbench-install.XXXXXX") \
    || die 'could not create a temporary directory.'

if [ "$VERSION" = latest ]; then
    RELEASE_JSON="$TMP_DIR/release.json"
    note "Resolving the latest release from $REPOSITORY."
    download "https://api.github.com/repos/$REPOSITORY/releases/latest" "$RELEASE_JSON"
    TAG=$(tr '\n' ' ' < "$RELEASE_JSON" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    [ -n "$TAG" ] || die 'could not read tag_name from the GitHub release response.'
else
    case "$VERSION" in
        v[0-9]*) TAG=$VERSION ;;
        [0-9]*) TAG="v$VERSION" ;;
        *) die "invalid release version: $VERSION" ;;
    esac
fi

case "$TAG" in
    v[0-9]*) ;;
    *) die "invalid GitHub release tag: $TAG" ;;
esac
case "$TAG" in
    *[!0-9A-Za-z._-]*) die "invalid GitHub release tag: $TAG" ;;
esac

ASSET_NAME="io-workbench-$TAG-$TARGET.tar.gz"
RELEASE_BASE="https://github.com/$REPOSITORY/releases/download/$TAG"
ARCHIVE="$TMP_DIR/$ASSET_NAME"
SUMS_FILE="$TMP_DIR/SHA256SUMS"

note "Downloading $ASSET_NAME."
download "$RELEASE_BASE/$ASSET_NAME" "$ARCHIVE"
download "$RELEASE_BASE/SHA256SUMS" "$SUMS_FILE"

EXPECTED_SHA256=$(awk -v filename="$ASSET_NAME" '$2 == filename || $2 == ("*" filename) { print $1; exit }' "$SUMS_FILE")
case "$EXPECTED_SHA256" in
    ????????*) ;;
    *) die "SHA256SUMS does not contain $ASSET_NAME." ;;
esac
case "$EXPECTED_SHA256" in
    *[!0123456789abcdefABCDEF]*) die "SHA256SUMS has an invalid hash for $ASSET_NAME." ;;
esac
[ "${#EXPECTED_SHA256}" -eq 64 ] \
    || die "SHA256SUMS has an invalid hash length for $ASSET_NAME."

ACTUAL_SHA256=$(sha256_file "$ARCHIVE")
EXPECTED_SHA256=$(printf '%s' "$EXPECTED_SHA256" | tr 'ABCDEF' 'abcdef')
ACTUAL_SHA256=$(printf '%s' "$ACTUAL_SHA256" | tr 'ABCDEF' 'abcdef')
[ "$ACTUAL_SHA256" = "$EXPECTED_SHA256" ] \
    || die "checksum verification failed for $ASSET_NAME."
note 'Release checksum verified.'

command -v tar >/dev/null 2>&1 || die 'tar is required to unpack the release.'
EXTRACT_DIR="$TMP_DIR/package"
mkdir -p "$EXTRACT_DIR"
tar -xzf "$ARCHIVE" -C "$EXTRACT_DIR"

for required_file in io-workbench iowb; do
    [ -f "$EXTRACT_DIR/$required_file" ] \
        || die "release archive is missing required file: $required_file"
done

BIN_DIR="${IO_WORKBENCH_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$BIN_DIR"
install -m 0755 "$EXTRACT_DIR/io-workbench" "$BIN_DIR/io-workbench"
install -m 0755 "$EXTRACT_DIR/iowb" "$BIN_DIR/iowb"

note "Installed io-workbench and iowb to $BIN_DIR."
CURRENT_PATH=$(printenv PATH 2>/dev/null || true)
case ":$CURRENT_PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        warn "$BIN_DIR is not on PATH in this shell."
        printf '%s\n' "Add this to your shell profile: export PATH=\"$BIN_DIR:\$PATH\""
        ;;
esac

printf '%s\n' \
    '' \
    'Start a local, authenticated workbench when you are ready:' \
    '  io-workbench start' \
    '' \
    'Then open http://127.0.0.1:8787 and complete first-user setup.' \
    'This installer does not start a background service or expose the host to the network.'
