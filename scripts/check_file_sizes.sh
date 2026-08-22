#!/usr/bin/env bash
# Quality gate: no tracked Rust file may exceed the line-count limit below.
# A file this long usually means several concerns are living together —
# split it into modules instead of raising the limit.
set -euo pipefail

max_lines=500
failed=0

while IFS= read -r -d '' file; do
    # Skip paths git still has staged/tracked but that were already moved on
    # disk (e.g. mid-refactor, before `git add` catches up).
    [ -f "$file" ] || continue
    lines=$(wc -l <"$file")
    if [ "$lines" -gt "$max_lines" ]; then
        echo "FAIL: $file has $lines lines (limit $max_lines)"
        failed=1
    fi
done < <(git ls-files -z -- '*.rs')

if [ "$failed" -ne 0 ]; then
    echo
    echo "One or more files exceed the ${max_lines}-line quality gate."
    exit 1
fi

echo "All tracked Rust files are within the ${max_lines}-line limit."
