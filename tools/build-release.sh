#!/usr/bin/env bash
# Builds Ossiframe Doctor and Ossiframe Deploy for macOS (.app and .dmg) and
# copies them into OUT_DIR with stable names, plus a SHA256SUMS file.
# Each app is collected right after its build, because building the next
# app's dmg recreates target/release/bundle/dmg and deletes the previous one.
#
# Usage: tools/build-release.sh OUT_DIR
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 OUT_DIR" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$1"
target="${CARGO_TARGET_DIR:-$root/target}"
version="$(jq -r .version "$root/apps/deploy/src-tauri/tauri.conf.json")"

mkdir -p "$out"

for app in Doctor Deploy; do
  pkg="@ossiframe/$(printf '%s' "$app" | tr '[:upper:]' '[:lower:]')"
  name="Ossiframe-${app}_${version}_aarch64"

  (cd "$root" && pnpm --filter "$pkg" tauri build --bundles app,dmg)

  bundle="$target/release/bundle/macos/Ossiframe ${app}.app"
  dmg="$target/release/bundle/dmg/Ossiframe ${app}_${version}_aarch64.dmg"
  for f in "$bundle" "$dmg"; do
    if [ ! -e "$f" ]; then
      echo "error: missing build output: $f" >&2
      exit 1
    fi
  done

  ditto -c -k --keepParent "$bundle" "$out/${name}.app.zip"
  cp "$dmg" "$out/${name}.dmg"
done

(cd "$out" && shasum -a 256 -- *.zip *.dmg > SHA256SUMS && cat SHA256SUMS)
