# AUR Publishing

Autolon publishes to AUR as `autolon-bin` first. This gives non-technical users the simplest install path:

```sh
yay -S autolon-bin
```

Later updates are picked up with:

```sh
yay -Syu
```

## Release Flow

From the repo root:

```sh
./packaging/release/build-binary-archive.sh
```

Upload the generated files from `dist/` to a GitHub release named `v0.1.0`:

```sh
gh release create v0.1.0 \
  dist/autolon-0.1.0-x86_64.tar.zst \
  dist/autolon-0.1.0-x86_64.tar.zst.sha256 \
  --title "Autolon v0.1.0" \
  --notes "Initial native Linux Wayland-first autoclicker release."
```

Then update and test the AUR package:

```sh
cd packaging/aur/autolon-bin
updpkgsums
makepkg --printsrcinfo > .SRCINFO
makepkg -si
autolon status
autolon permissions status
autolon verify
```

## Publish to AUR

One-time setup:

```sh
mkdir -p ~/.ssh
ssh-keyscan aur.archlinux.org >> ~/.ssh/known_hosts
git clone ssh://aur@aur.archlinux.org/autolon-bin.git /tmp/autolon-bin-aur
```

Publish:

```sh
cp packaging/aur/autolon-bin/PKGBUILD \
   packaging/aur/autolon-bin/.SRCINFO \
   packaging/aur/autolon-bin/autolon.install \
   /tmp/autolon-bin-aur/
cd /tmp/autolon-bin-aur
git add PKGBUILD .SRCINFO autolon.install
git commit -m "Release 0.1.0-1"
git push
```

After that, a fresh machine can install with:

```sh
yay -S autolon-bin
```
