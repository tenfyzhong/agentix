#!/bin/sh

set -eu

release_tag="${1:-}"

if [ -z "$release_tag" ]; then
    echo "usage: release-version.sh <tag>" >&2
    exit 2
fi

version="${release_tag#v}"
if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'; then
    echo "release tag '$release_tag' is not a supported semantic version tag" >&2
    exit 1
fi

printf '%s\n' "$version"
