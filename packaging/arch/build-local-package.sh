#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
pkgname="autolon"
pkgver="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_root/Cargo.toml" | head -n 1)"

if [[ -z "$pkgver" ]]; then
  echo "Could not read package version from Cargo.toml" >&2
  exit 1
fi

workdir="$(mktemp -d "${TMPDIR:-/tmp}/autolon-pkg.XXXXXX")"
archive="$script_dir/$pkgname-$pkgver.tar.gz"

cleanup() {
  rm -rf "$workdir"
  rm -f "$archive"
}
trap cleanup EXIT

stage="$workdir/$pkgname-$pkgver"
mkdir -p "$stage"
rsync -a \
  --exclude='/.git' \
  --exclude='/target' \
  --exclude='/packaging/arch/pkg' \
  --exclude='/packaging/arch/src' \
  --exclude='/packaging/arch/*.tar.*' \
  --exclude='/packaging/arch/*.pkg.tar.*' \
  "$repo_root/" "$stage/"

tar -C "$workdir" -czf "$archive" "$pkgname-$pkgver"

makepkg_args=(--syncdeps --install --clean --force)
if [[ "$#" -gt 0 ]]; then
  makepkg_args=("$@")
fi

cd "$script_dir"
makepkg "${makepkg_args[@]}"

if [[ "$#" -eq 0 ]]; then
  status_line="Autolon package install finished."
  executable_line="Installed executable:"
else
  status_line="Autolon package command finished."
  executable_line="Package install target:"
fi

cat <<EOF

$status_line

$executable_line
  /usr/bin/autolon

Next checks:
  hash -r
  autolon permissions status
  autolon verify
EOF
