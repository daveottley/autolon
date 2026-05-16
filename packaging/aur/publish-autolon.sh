#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
pkgdir="$repo_root/packaging/aur/autolon"
aur_workdir="${1:-/tmp/autolon-aur}"

aur_auth_output="$(ssh -o BatchMode=yes -T aur@aur.archlinux.org 2>&1 || true)"
grep -Eq 'Welcome|Hi|successfully authenticated' <<<"$aur_auth_output" || {
  cat >&2 <<'EOF'
Could not authenticate to AUR over SSH.

Add your public key to your AUR account, then rerun this script:
  cat ~/.ssh/id_ed25519.pub

AUR account page:
  https://aur.archlinux.org/account
EOF
  printf '%s\n' "$aur_auth_output" >&2
  exit 1
}

rm -rf "$aur_workdir"
git clone ssh://aur@aur.archlinux.org/autolon.git "$aur_workdir"
git -C "$aur_workdir" checkout -B master
cp "$pkgdir/PKGBUILD" "$pkgdir/.SRCINFO" "$pkgdir/autolon.install" "$aur_workdir/"

cd "$aur_workdir"
git add PKGBUILD .SRCINFO autolon.install
if git diff --cached --quiet; then
  echo "No AUR package changes to publish."
  exit 0
fi

pkgver="$(sed -n 's/^pkgver=//p' "$pkgdir/PKGBUILD" | head -n 1)"
pkgrel="$(sed -n 's/^pkgrel=//p' "$pkgdir/PKGBUILD" | head -n 1)"
git commit -m "Release $pkgver-$pkgrel"
git push origin master

cat <<'EOF'
Published autolon to AUR.

Source install:
  yay -S autolon
EOF
