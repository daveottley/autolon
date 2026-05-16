#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
pkgver="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_root/Cargo.toml" | head -n 1)"
arch="${CARCH:-$(uname -m)}"

if [[ -z "$pkgver" ]]; then
  echo "Could not read package version from Cargo.toml" >&2
  exit 1
fi

if [[ "$arch" != "x86_64" ]]; then
  echo "Only x86_64 release archives are supported right now; got $arch" >&2
  exit 1
fi

dist="$repo_root/dist"
workdir="$(mktemp -d "${TMPDIR:-/tmp}/autolon-release.XXXXXX")"
root_name="autolon-$pkgver-$arch"
root="$workdir/$root_name"
archive="$dist/$root_name.tar.zst"

cleanup() {
  rm -rf "$workdir"
}
trap cleanup EXIT

cd "$repo_root"
export RUSTFLAGS="${AUTOLON_RELEASE_RUSTFLAGS:--C target-cpu=x86-64}"
cargo build --release --locked

disassembly="$workdir/autolon.objdump"
objdump -d -M intel target/release/autolon > "$disassembly"
bad_instructions="$(
  awk 'tolower($0) ~ /(zmm[0-9]+|vpermt2d|vpmovm2|vpternlog|vshufi64x2|vextracti64x4)/ { print; if (++count == 20) exit }' "$disassembly"
)"
if [[ -n "$bad_instructions" ]]; then
  cat >&2 <<'EOF'
Release binary contains AVX-512-family instructions.
Refusing to publish a non-portable x86_64 binary archive.
EOF
  printf '%s\n' "$bad_instructions" >&2
  exit 1
fi

install -Dm755 target/release/autolon "$root/usr/bin/autolon"
install -Dm644 packaging/linux/io.github.autolon.Autolon.desktop \
  "$root/usr/share/applications/io.github.autolon.Autolon.desktop"
install -Dm644 packaging/linux/io.github.autolon.Autolon.svg \
  "$root/usr/share/icons/hicolor/scalable/apps/io.github.autolon.Autolon.svg"
install -Dm644 packaging/linux/io.github.autolon.Autolon.metainfo.xml \
  "$root/usr/share/metainfo/io.github.autolon.Autolon.metainfo.xml"
install -Dm644 packaging/linux/autolon.service \
  "$root/usr/lib/systemd/user/autolon.service"
install -Dm644 packaging/linux/70-autolon-uinput.rules \
  "$root/usr/lib/udev/rules.d/70-autolon-uinput.rules"
install -Dm644 packaging/linux/autolon.sysusers \
  "$root/usr/lib/sysusers.d/autolon.conf"
install -Dm644 LICENSE "$root/usr/share/licenses/autolon/LICENSE"

mkdir -p "$dist"
rm -f "$archive" "$archive.sha256"
tar -C "$workdir" --zstd -cf "$archive" "$root_name"
sha256sum "$archive" > "$archive.sha256"

cat <<EOF
Release archive:
  $archive

Checksum:
  $(cut -d' ' -f1 "$archive.sha256")
EOF
