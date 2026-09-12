#!/bin/sh
# Reusable fake CLI: assert the real v4 command contract, keep server state outside
# Agentix's temporary Slack project, and never depend on user credentials.
set -eu
fixture_dir=$(dirname "$0")
pwd >> "$fixture_dir/projects"
printf '%s\n' "$*" >> "$fixture_dir/calls"
case "$*" in *--token*) exit 81 ;; esac
[ -f .slack/hooks.json ] || exit 82
[ -f .slack/config.json ] || exit 83
if [ -f "$fixture_dir/validation-fail" ]; then
  printf "private-token (desc_too_long)\n" >&2
  exit 1
fi
if [ -f "$fixture_dir/fail" ]; then
  printf 'secret-token-must-not-appear\n' >&2
  exit 7
fi
if [ -f "$fixture_dir/hang" ]; then
  printf 'ready\n' > "$fixture_dir/hang-ready"
  exec sleep 30
fi
case "$1 $2" in
  'app link')
    [ ! -e manifest.json ] || exit 88
    [ "$*" = 'app link --environment deployed --app A123 --team T123 --no-color --skip-update' ] || exit 84
    ;;
  'manifest info')
    if [ -f "$fixture_dir/missing-app" ]; then
      printf 'private-token (app_not_found)\n' >&2
      exit 1
    fi
    [ "$*" = 'manifest info --source remote --app A123 --team T123 --no-color --skip-update' ] || exit 85
    if [ -f "$fixture_dir/race" ] && [ -f "$fixture_dir/fetched" ]; then
      cat "$fixture_dir/changed.json"
    else
      cat "$fixture_dir/remote.json"
    fi
    touch "$fixture_dir/fetched"
    ;;
  'app install')
    [ -f manifest.json ] || exit 89
    [ "$*" = 'app install --force --app A123 --team T123 --no-color --skip-update' ] || exit 86
    if [ -f "$fixture_dir/install-fail" ]; then exit 9; fi
    if [ -f "$fixture_dir/ignore-install" ]; then exit 0; fi
    cp manifest.json "$fixture_dir/remote.json"
    cp manifest.json "$fixture_dir/installed.json"
    ;;
  *) exit 87 ;;
esac
