#!/usr/bin/env bash
# Build a universal (Apple Silicon + Intel) Mailvern.dmg and publish it as a GitHub Release.
#
#   npm run release           build, then create release v<version> with the .dmg attached
#   npm run release -- --build-only   build the .dmg without publishing
#
# The version comes from src-tauri/tauri.conf.json. Bump it (and package.json and
# src-tauri/Cargo.toml) in a commit before releasing.
set -euo pipefail

cd "$(dirname "$0")/.."
# rustup's cargo isn't always on PATH on this Mac.
[ -d /opt/homebrew/opt/rustup/bin ] && export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

build_only=false
[ "${1:-}" = "--build-only" ] && build_only=true

version=$(node -p 'require("./src-tauri/tauri.conf.json").version')
tag="v$version"
target=universal-apple-darwin
dmg="target/$target/release/bundle/dmg/Mailvern_${version}_universal.dmg"

die() { echo "error: $*" >&2; exit 1; }

for f in package.json src-tauri/Cargo.toml; do
  grep -Eq "^ *\"?version\"? *[:=] *\"$version\"" "$f" || die "$f doesn't have version $version (tauri.conf.json does)"
done

if ! $build_only; then
  command -v gh >/dev/null || die "the GitHub CLI (gh) is needed to publish"
  [ -z "$(git status --porcelain)" ] || die "commit or stash your changes first"
  [ "$(git branch --show-current)" = main ] || die "release from main"
  git fetch -q origin main
  [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "push main first (HEAD isn't origin/main)"
  ! gh release view "$tag" >/dev/null 2>&1 || die "release $tag already exists; bump the version"
fi

rustup target list --installed | grep -q x86_64-apple-darwin || rustup target add x86_64-apple-darwin
rustup target list --installed | grep -q aarch64-apple-darwin || rustup target add aarch64-apple-darwin

echo "Building Mailvern $version ($target)…"
npm run tauri build -- --target "$target"
[ -f "$dmg" ] || die "build finished but $dmg is missing"
echo "Built $dmg ($(du -h "$dmg" | cut -f1))"

$build_only && exit 0

read -r -p "Publish $tag to GitHub Releases from $(git rev-parse --short HEAD)? [y/N] " ok
[ "$ok" = y ] || [ "$ok" = Y ] || { echo "Not published."; exit 0; }

gh release create "$tag" "$dmg" \
  --target "$(git rev-parse HEAD)" \
  --title "Mailvern $version" \
  --generate-notes
