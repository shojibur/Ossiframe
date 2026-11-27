#!/usr/bin/env bash
# Checks that the workspace Cargo.toml, both tauri.conf.json files and both
# app package.json files share one version. With an argument (a tag such as
# v0.1.0), also checks that the tag is "v" followed by that version.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

cargo_version="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p;}' "$root/Cargo.toml")"
if [ -z "$cargo_version" ]; then
  echo "error: no version in [workspace.package] of Cargo.toml" >&2
  exit 1
fi

status=0
for file in \
  apps/doctor/src-tauri/tauri.conf.json \
  apps/deploy/src-tauri/tauri.conf.json \
  apps/doctor/package.json \
  apps/deploy/package.json; do
  v="$(jq -r '.version // empty' "$root/$file")"
  if [ "$v" != "$cargo_version" ]; then
    echo "error: $file has version '$v', Cargo.toml has '$cargo_version'" >&2
    status=1
  fi
done

if [ "$#" -ge 1 ] && [ "$1" != "v$cargo_version" ]; then
  echo "error: tag '$1' does not match version v$cargo_version" >&2
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "version $cargo_version is consistent"
fi
exit "$status"
