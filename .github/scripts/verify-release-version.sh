#!/bin/sh

set -eu

release_tag="${1:-}"
manifest="${2:-Cargo.toml}"

if [ -z "$release_tag" ]; then
    echo "usage: verify-release-version.sh <tag> [Cargo.toml]" >&2
    exit 2
fi

tag_version="${release_tag#v}"
if ! printf '%s\n' "$tag_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'; then
    echo "release tag '$release_tag' is not a supported semantic version tag" >&2
    exit 1
fi

workspace_version="$(awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    in_workspace_package && /^\[/ { exit }
    in_workspace_package && /^[[:space:]]*version[[:space:]]*=/ {
        value = $0
        sub(/^[^"]*"/, "", value)
        sub(/".*/, "", value)
        print value
        exit
    }
' "$manifest")"

if [ -z "$workspace_version" ]; then
    echo "could not read workspace.package.version from $manifest" >&2
    exit 1
fi

if [ "$tag_version" != "$workspace_version" ]; then
    echo "release tag version '$tag_version' does not match workspace version '$workspace_version'" >&2
    exit 1
fi

printf '%s\n' "$workspace_version"
