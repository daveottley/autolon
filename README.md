# Autolon

Autolon is an autoclicker and local automation controller for Legends of IdleOn on Linux.

It is built for a narrow, practical job: keep click automation local, visible, configurable, and easy to stop. Autolon does not read game memory, alter packets, bypass detection, or perform network automation.

**Platform focus:** Wayland-only, KDE preferred.

Autolon is currently most comfortable on KDE Plasma Wayland. Some fallback paths exist, but users on other desktops should expect rougher behavior, especially for global hotkeys and pointer overlays.

## Features

- Three speed slots: `Slow`, `Fast`, and `User`
- Global cycle hotkey, default `F6`
- Emergency stop hotkey, default `F7`
- Adjustable hotkey debounce for fast repeated key presses
- GTK settings window with a local test canvas
- Optional global pointer overlay showing click state and speed
- Color-coded click-speed indicator
- System tray integration
- User service support for a resident autoclicker daemon
- Wayland pointer injection through `/dev/uinput`
- Direct keyboard grab fallback for reliable global hotkeys when permissions allow it

## Install From AUR

On Arch Linux, CachyOS, or another Arch-based system with an AUR helper:

```sh
yay -S autolon
```

This builds Autolon locally on your machine. It is the recommended package if you want the binary compiled for your system.

For the prebuilt package:

```sh
yay -S autolon-bin
```

Use either `autolon` or `autolon-bin`, not both. They conflict with each other intentionally because they install the same program.

To switch from the prebuilt package to the locally built package:

```sh
yay -Rns autolon-bin
yay -S autolon
```

To switch from the locally built package to the prebuilt package:

```sh
yay -Rns autolon
yay -S autolon-bin
```

Avoid using `yay -Syu autolon` as a package-switch command. `-Syu` means "upgrade everything and install this target," so an AUR helper may show an `autolon-bin` update before resolving the package conflict. Remove the installed variant first for a clean, predictable switch.

After install, refresh your shell and verify the setup:

```sh
hash -r
autolon permissions status
autolon verify
autolon gui
```

If `direct_keyboard_grab_ready` is still `false`, open a new terminal or log out and back in once, then run:

```sh
autolon verify
```

## Install From This Repository

From a fresh checkout on Arch or CachyOS:

```sh
./packaging/arch/build-local-package.sh
```

That installs:

- `/usr/bin/autolon`
- the desktop launcher
- the application icon
- metainfo metadata
- the systemd user service
- the udev input permission rule
- the `autolon-input` sysusers group definition

The package install hook reloads udev, retriggers input devices, adds the installing user to `autolon-input`, and applies current-session ACLs when possible.

## Build For Development

```sh
cargo build --release
```

The development binary is written to:

```text
target/release/autolon
```

This build path is useful while developing, but it does not install Autolon onto your `PATH` and does not install the Wayland input permission rules.

## First Run

Start the daemon:

```sh
autolon daemon
```

Open the settings window:

```sh
autolon gui
```

Useful checks:

```sh
autolon status
autolon verify
autolon permissions status
```

`autolon status` is read-only. It reports an existing daemon if one is running, or `state: daemon not running` if not.

## Basic Controls

Default hotkeys:

```text
F6  cycle autoclick speed
F7  emergency stop
```

Default cycle order:

```text
Off -> Slow -> Fast -> User -> Off
```

The settings window lets you adjust slot speeds, enable or disable slots, tune debounce timing, test clicks on a local canvas, and enable global hotkeys.

## Command Line Examples

```sh
autolon cycle
autolon stop
autolon slot set 1 --interval-ms 500
autolon slot set 2 --interval-ms 10
autolon slot set 3 --interval-ms 1000
autolon test-global-hotkey --seconds 20
```

The default config file is created here:

```text
~/.config/autolon/config.toml
```

## Wayland Permissions

Wayland does not allow ordinary applications to inject pointer events into other applications. Autolon uses Linux virtual input through `/dev/uinput`.

For packaged installs, the udev rule is installed automatically. For source-tree testing, install it once:

```sh
autolon permissions install
```

Manual equivalent:

```sh
sudo install -Dm644 packaging/linux/70-autolon-uinput.rules /usr/lib/udev/rules.d/70-autolon-uinput.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
```

If permissions do not refresh immediately, log out and back in. If your distro does not apply `uaccess` to input devices, use the group fallback:

```sh
sudo groupadd -r autolon-input 2>/dev/null || true
sudo usermod -aG autolon-input "$USER"
```

Then log out and back in.

## Global Overlay

Autolon includes an optional global mouse overlay that shows when autoclicking is active and displays the current speed.

This feature is experimental and KDE-focused. It is disabled by default because global overlay behavior can vary across Wayland compositors. The packaged installs depend on `qt6-tools` for the KDE D-Bus bridge used by the overlay. The local test canvas always remains available for safe testing inside the settings window.

## Desktop Integration

User-level helpers:

```sh
autolon autostart enable
autolon autostart disable
autolon desktop-icon install
autolon desktop-icon remove
```

Install desktop files into a staging prefix:

```sh
autolon install-desktop-files --prefix /tmp/autolon-root/usr
```

## Safety Notes

Autolon is meant for transparent local input automation. It should always be obvious when it is active, and `F7` should be treated as the first stop button to test.

Before using automation with any game account, verify that your use case is allowed.

## License

MIT. See [LICENSE](LICENSE).
