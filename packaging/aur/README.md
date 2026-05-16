# AUR Publishing

Autolon publishes two AUR packages:

- `autolon-bin`: downloads the portable binary archive from GitHub Releases.
- `autolon`: builds from source on the installing machine with native CPU optimization.

The binary package gives non-technical users the simplest install path:

```sh
yay -S autolon-bin
```

The source package is slower to install, but compiles for the local CPU:

```sh
yay -S autolon
```

Later updates are picked up with:

```sh
yay -Syu
```

## Release Flow

Binary archives are built by GitHub Actions when a `v*` tag is pushed. The workflow uses the official Rust toolchain and refuses to publish an archive if the resulting binary contains AVX/AVX-512-family instructions that would make it unsafe as a portable `x86_64` build.

Manual local archive builds are still available from the repo root:

```sh
./packaging/release/build-binary-archive.sh
```

For normal releases, tag and push:

```sh
git tag v0.1.2
git push origin v0.1.2
```

After GitHub Actions publishes the release assets, update and test the binary AUR package:

```sh
cd packaging/aur/autolon-bin
updpkgsums
makepkg --printsrcinfo > .SRCINFO
makepkg -si
autolon status
autolon permissions status
autolon verify
```

Then update and test the source AUR package:

```sh
cd packaging/aur/autolon
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
cat ~/.ssh/id_ed25519.pub
```

Add that public key to your AUR account:

```text
https://aur.archlinux.org/account
```

Then publish both packages:

```sh
./packaging/aur/publish-autolon-bin.sh
./packaging/aur/publish-autolon.sh
```

Manual publish equivalent:

```sh
git clone ssh://aur@aur.archlinux.org/autolon-bin.git /tmp/autolon-bin-aur
cp packaging/aur/autolon-bin/PKGBUILD \
   packaging/aur/autolon-bin/.SRCINFO \
   packaging/aur/autolon-bin/autolon.install \
   /tmp/autolon-bin-aur/
cd /tmp/autolon-bin-aur
git add PKGBUILD .SRCINFO autolon.install
git commit -m "Release 0.1.2-1"
git push
```

After that, a fresh machine can install with:

```sh
yay -S autolon-bin
```
