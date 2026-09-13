#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
: "${FORMULA:?}" "${FORMULA_PATH:?}" "${PREBUILT_BINARY:?}" "${TAP_NAME:?}" "${RELEASE_TAG:?}" "${BOTTLE_ROOT_URL:?}"
version="$("$script_dir/release-version.sh" "$RELEASE_TAG")"
[[ "$("$PREBUILT_BINARY" --version)" == "$FORMULA $version" ]] || {
    echo "Prebuilt binary version does not match $FORMULA $version" >&2
    exit 1
}
qualified="$TAP_NAME/$FORMULA"
if brew list --versions "$qualified" >/dev/null 2>&1; then
    echo "Refusing to replace an existing installation of $qualified" >&2
    exit 1
fi
source_formula="$(mktemp)"
cp "$FORMULA_PATH" "$source_formula"
tap_formula_path="$(brew --repository "$TAP_NAME")/Formula/$FORMULA.rb"
restore_formula() {
    cp "$source_formula" "$tap_formula_path"
    rm -f "$source_formula"
}
trap restore_formula EXIT
cp "$source_formula" "$tap_formula_path"
FORMULA_PATH="$tap_formula_path" ruby "$script_dir/prepare-bottle-formula.rb"
brew install --build-bottle "$qualified"
# Restore the public source recipe, including build dependencies, before export.
cp "$source_formula" "$tap_formula_path"
keg="$(brew --prefix "$qualified")"
cp "$source_formula" "$keg/.brew/$FORMULA.rb"
brew linkage --test "$qualified"
brew test "$qualified"
brew bottle --json --no-rebuild --root-url "$BOTTLE_ROOT_URL" "$qualified"
shopt -s nullglob
bottles=("$FORMULA"--*.bottle.tar.gz)
[[ "${#bottles[@]}" -eq 1 ]] || { echo "Expected exactly one bottle" >&2; exit 1; }
# Test the exported bottle, not only the staging installation.
brew uninstall --force "$qualified"
# Homebrew restricts local package paths to developer/test invocations.
HOMEBREW_DEVELOPER=1 brew install --force-bottle "$PWD/${bottles[0]}"
[[ "$("$(brew --prefix "$qualified")/bin/$FORMULA" --version)" == "$FORMULA $version" ]]
brew linkage --test "$qualified"
brew test "$qualified"
bash "$script_dir/normalize-homebrew-bottle.sh"
