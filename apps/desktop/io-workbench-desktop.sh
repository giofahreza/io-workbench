#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
BIN="${IO_WORKBENCH_DESKTOP_BIN:-$ROOT_DIR/target/release/io-workbench}"
HOST="${IO_WORKBENCH_HOST:-127.0.0.1}"
PORT="${IO_WORKBENCH_PORT:-8787}"
URL="http://$HOST:$PORT"

if [ ! -x "$BIN" ]; then
  echo "Missing executable: $BIN" >&2
  echo "Run: cargo build --release -p iowb-cli --bin io-workbench" >&2
  exit 1
fi

"$BIN" start &
SERVER_PID="$!"

cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

i=0
while [ "$i" -lt 60 ]; do
  if curl -fsS "$URL/health" >/dev/null 2>&1; then
    break
  fi
  i=$((i + 1))
  sleep 0.25
done

if command -v xdg-open >/dev/null 2>&1; then
  xdg-open "$URL" >/dev/null 2>&1 || true
elif command -v open >/dev/null 2>&1; then
  open "$URL" >/dev/null 2>&1 || true
elif command -v start >/dev/null 2>&1; then
  start "$URL" >/dev/null 2>&1 || true
fi

wait "$SERVER_PID"
