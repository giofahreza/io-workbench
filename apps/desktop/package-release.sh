#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
VERSION="${IO_WORKBENCH_PACKAGE_VERSION:-0.1.0}"
TARGET_DIR="$ROOT_DIR/target/release"
DIST_DIR="$ROOT_DIR/dist"
BIN="$TARGET_DIR/io-workbench"

if [ ! -x "$BIN" ]; then
  cargo build --release -p iowb-cli --bin io-workbench
fi

mkdir -p "$DIST_DIR"

package_archive() {
  os="$1"
  ext="$2"
  work="$DIST_DIR/io-workbench-$VERSION-$os"
  rm -rf "$work"
  mkdir -p "$work"
  cp "$BIN" "$work/io-workbench"
  cp "$ROOT_DIR/apps/desktop/io-workbench-desktop.sh" "$work/io-workbench-desktop.sh"
  cp "$ROOT_DIR/apps/desktop/desktop-package.json" "$work/desktop-package.json"
  cp "$ROOT_DIR/README.md" "$work/README.md"
  if [ "$ext" = "zip" ]; then
    (cd "$DIST_DIR" && zip -q -r "io-workbench-$VERSION-$os.zip" "io-workbench-$VERSION-$os")
  else
    (cd "$DIST_DIR" && tar -czf "io-workbench-$VERSION-$os.tar.gz" "io-workbench-$VERSION-$os")
  fi
  rm -rf "$work"
}

package_archive "linux-x64" "tar.gz"
package_archive "macos-universal" "tar.gz"
package_archive "windows-x64" "zip"

printf '%s\n' "$DIST_DIR/io-workbench-$VERSION-linux-x64.tar.gz"
printf '%s\n' "$DIST_DIR/io-workbench-$VERSION-macos-universal.tar.gz"
printf '%s\n' "$DIST_DIR/io-workbench-$VERSION-windows-x64.zip"
