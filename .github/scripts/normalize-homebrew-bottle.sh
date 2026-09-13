#!/usr/bin/env bash
set -euo pipefail
: "${FORMULA:?}"
shopt -s nullglob
bottles=("$FORMULA"--*.bottle.tar.gz)
if [[ "${#bottles[@]}" -ne 1 ]]; then
    echo "Expected exactly one Homebrew bottle archive for $FORMULA" >&2
    exit 1
fi
mv "${bottles[0]}" "${bottles[0]/$FORMULA--/$FORMULA-}"
