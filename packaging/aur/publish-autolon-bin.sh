#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
pkgdir="$repo_root/packaging/aur/autolon-bin"
aur_workdir="${1:-/tmp/autolon-bin-aur}"

ssh -o BatchMode=yes -T aur@aur.archlinux.org 2>&1 | grep -Eq 'Welcome|Hi|successfully authenticated' || {
  cat >&2 <<'EOF'
Could not authenticate to AUR over SSH.

Add your public key to your AUR account, then rerun this script:
  cat ~/.ssh/id_ed25519.pub

AUR account page:
  https://aur.archlinux.org/account
EOF
  exit 1
}

rm -rf "$aur_workdir"
git clone ssh://aur@aur.archlinux.org/autolon-bin.git "$aur_workdir"
cp "$pkgdir/PKGBUILD" "$pkgdir/.SRCINFO" "$pkgdir/autolon.install" "$aur_workdir/"

cd "$aur_workdir"
git add PKGBUILD .SRCINFO autolon.install
if git diff --cached --quiet; then
  echo "No AUR package changes to publish."
  exit 0
fi

git commit -m "Release 0.1.0-1"
git push

cat <<'EOF'
Published autolon-bin to AUR.

Fresh install:
  yay -S autolon-bin

Updates:
  yay -Syu
EOF
