#!/bin/sh

set -eu

release_tag="${1:-}"
manifest="${2:-Cargo.toml}"
lockfile="${3:-Cargo.lock}"
script_directory="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
version="$("$script_directory/release-version.sh" "$release_tag")"

if [ ! -f "$manifest" ]; then
    echo "workspace manifest '$manifest' does not exist" >&2
    exit 1
fi
if [ ! -f "$lockfile" ]; then
    echo "workspace lockfile '$lockfile' does not exist" >&2
    exit 1
fi

repository_directory="$(dirname -- "$manifest")"
workspace_packages=""
workspace_package_count=0
for package_manifest in "$repository_directory"/crates/*/Cargo.toml; do
    [ -f "$package_manifest" ] || continue
    package_name="$(awk '
        /^\[package\]$/ { in_package = 1; next }
        in_package && /^\[/ { exit }
        in_package && /^[[:space:]]*name[[:space:]]*=/ {
            value = $0
            sub(/^[^"]*"/, "", value)
            sub(/".*/, "", value)
            print value
            exit
        }
    ' "$package_manifest")"
    if [ -z "$package_name" ]; then
        echo "could not read package name from '$package_manifest'" >&2
        exit 1
    fi
    workspace_packages="$workspace_packages $package_name"
    workspace_package_count=$((workspace_package_count + 1))
done

if [ "$workspace_package_count" -eq 0 ]; then
    echo "workspace contains no package manifests under '$repository_directory/crates'" >&2
    exit 1
fi

manifest_output="$(mktemp "${manifest}.XXXXXX")"
lockfile_output="$(mktemp "${lockfile}.XXXXXX")"
cleanup() {
    rm -f "$manifest_output" "$lockfile_output"
}
trap cleanup EXIT HUP INT TERM

awk -v version="$version" '
    /^\[workspace\.package\]$/ {
        in_workspace_package = 1
        print
        next
    }
    in_workspace_package && /^\[/ {
        in_workspace_package = 0
    }
    in_workspace_package && /^[[:space:]]*version[[:space:]]*=/ {
        line = $0
        sub(/"[^"]*"/, "\"" version "\"", line)
        print line
        updated = 1
        next
    }
    { print }
    END {
        if (!updated) {
            exit 1
        }
    }
' "$manifest" > "$manifest_output"

awk \
    -v version="$version" \
    -v workspace_packages="$workspace_packages " \
    -v expected="$workspace_package_count" '
    /^\[\[package\]\]$/ {
        in_package = 1
        update_package = 0
    }
    in_package && /^name = / {
        name = $0
        sub(/^[^"]*"/, "", name)
        sub(/".*/, "", name)
        update_package = index(workspace_packages, " " name " ") > 0
    }
    in_package && update_package && /^version = / {
        line = $0
        sub(/"[^"]*"/, "\"" version "\"", line)
        print line
        update_package = 0
        updated += 1
        next
    }
    { print }
    END {
        if (updated != expected) {
            exit 1
        }
    }
' "$lockfile" > "$lockfile_output"

mv "$manifest_output" "$manifest"
mv "$lockfile_output" "$lockfile"
trap - EXIT HUP INT TERM
printf '%s\n' "$version"
