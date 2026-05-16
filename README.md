# Autolon

Autolon is a native Linux autoclicker and local input automation controller for Legends of IdleOn workflows.

The v0 scope is intentionally narrow:

- native Rust executable named `autolon`
- resident daemon with one owned click loop
- CLI commands for status, cycle, stop, slot speed, autostart, and desktop icon installation
- GTK settings window with a local click-speed test canvas
- three autoclicker slots: Slow, Fast, User
- cycle behavior: `Off -> Slow -> Fast -> User -> Off`
- local canvas shortcuts always work while focused: `F6` cycles, `F7` stops
- optional global shortcuts through KDE/portal first, with a direct `/dev/input` fallback: `F6` cycles, `F7` stops
- Wayland-first pointer injection through `/dev/uinput`
- X11 compatibility through XTest

Autolon is for transparent local input automation only. It does not do game memory reads, packet manipulation, stealth behavior, anti-detection bypasses, or network automation. Verify whether automation is allowed for your game account and use case.

## Install

On Arch/CachyOS, build and install the package from this checkout:

```sh
./packaging/arch/build-local-package.sh
```

That installs an executable at `/usr/bin/autolon`, so you do not use `./autolon`. It also installs the KDE launcher entry, icon, metainfo, systemd user service, udev input rule, and `autolon-input` sysusers group definition. The install hook reloads udev, retriggers input devices, adds the installing user to `autolon-input`, and applies current-session ACLs when possible.

After install:

```sh
hash -r
autolon permissions status
autolon verify
autolon gui
```

If `direct_keyboard_grab_ready` is still `false`, open a new terminal or log out and back in once, then run `autolon verify` again.

## Build

```sh
cargo build --release
```

The binary is written to `target/release/autolon`. This build path is useful for development, but it does not install `autolon` onto your `PATH` or install Wayland input permissions.

## Run

Start the daemon:

```sh
autolon daemon
```

Control it from another terminal:

```sh
autolon status
autolon verify
autolon permissions status
autolon permissions install
autolon test-global-hotkey --seconds 20
autolon cycle
autolon stop
autolon slot set 1 --interval-ms 500
autolon slot set 2 --interval-ms 10
autolon slot set 3 --interval-ms 1000
autolon gui
```

`autolon status` is read-only: it reports an existing daemon if one is running, or `state: daemon not running` if not. It does not start the daemon or create a tray icon.

The default config is created at:

```text
~/.config/autolon/config.toml
```

## Wayland Input Permissions

Wayland does not allow arbitrary apps to inject pointer events. Autolon v0 treats Wayland as first-class by using Linux virtual input through `/dev/uinput`.

Global F6/F7 hotkeys use direct `/dev/input/event*` keyboard grabbing when permission is available, because that can consume F6 before Chrome receives it. KDE Global Shortcuts and the desktop GlobalShortcuts portal are fallback paths. The packaged udev rule grants direct input access to the active local desktop user with `uaccess` and also supports an `autolon-input` group fallback.

For packaged installs, the package manager installs the included udev rule. When testing from a source checkout, install it once:

```sh
autolon permissions install
```

The Arch package also runs `packaging/arch/autolon.install` at install/upgrade time. That script creates the `autolon-input` group, reloads udev rules, and retriggers input devices so Autolon should not need a runtime permission prompt.

Or install it manually:

```sh
sudo install -Dm644 packaging/linux/70-autolon-uinput.rules /usr/lib/udev/rules.d/70-autolon-uinput.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Log out and back in if the ACL does not refresh immediately. If your distro does not apply `uaccess` to input devices, create an `autolon-input` group and add your user to it:

```sh
sudo groupadd -r autolon-input 2>/dev/null || true
sudo usermod -aG autolon-input "$USER"
```

## Desktop Integration

Install desktop files into a staging prefix:

```sh
autolon install-desktop-files --prefix /tmp/autolon-root/usr
```

User-level helpers:

```sh
autolon autostart enable
autolon autostart disable
autolon desktop-icon install
autolon desktop-icon remove
```

## Current Limitation

KDE and portal global shortcuts can trigger Autolon, but they do not provide the same guarantee as a physical keyboard grab: applications such as Chrome may still observe F6. The direct keyboard-grab path is required for the Chrome override test. Check it with:

```sh
autolon permissions status
```

`direct_keyboard_grab_ready` must be `true` before Autolon can reliably consume F6 ahead of Chrome. Global click injection also requires write access to `/dev/uinput`.
