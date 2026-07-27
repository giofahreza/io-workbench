#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
VERSION="${IO_WORKBENCH_PACKAGE_VERSION:-0.1.0}"
DIST_DIR="$ROOT_DIR/dist"
WORK="$DIST_DIR/io-workbench-mobile-$VERSION"

mkdir -p "$DIST_DIR"
rm -rf "$WORK"
mkdir -p "$WORK"
cp -R "$ROOT_DIR/apps/mobile/www/." "$WORK/"
cp "$ROOT_DIR/apps/mobile/README.md" "$WORK/README.md"

(cd "$DIST_DIR" && zip -q -r "io-workbench-mobile-$VERSION-pwa.zip" "io-workbench-mobile-$VERSION")
rm -rf "$WORK"

printf '%s\n' "$DIST_DIR/io-workbench-mobile-$VERSION-pwa.zip"
