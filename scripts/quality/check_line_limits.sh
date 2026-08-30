#!/usr/bin/env bash

set -euo pipefail

max_lines=2000
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
failed=0

# These are the first-party sources assembled into the port-8787 product.
# Vendored assets, generated dist output, and the separate apps submodule are
# intentionally outside this check and should be maintained by their owners.
while IFS= read -r -d '' file; do
    lines=$(wc -l < "$file")
    if (( lines > max_lines )); then
        printf '%s: %d lines (limit %d)\n' "${file#"$repo_root"/}" "$lines" "$max_lines" >&2
        failed=1
    fi
done < <(
    find "$repo_root/crates" "$repo_root/rag" \
        -path '*/vendor' -prune -o \
        -type f \( \
            -name '*.rs' -o \
            -name '*.js' -o \
            -name '*.css' -o \
            -name '*.html' -o \
            -name '*.ts' -o \
            -name '*.tsx' -o \
            -name '*.jsx' \
        \) -print0
)

if (( failed != 0 )); then
    exit 1
fi

printf 'Line-limit check passed: all first-party 8787 source files are at most %d lines.\n' "$max_lines"
