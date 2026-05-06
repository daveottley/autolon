# Arch Package

Build and install Autolon from this checkout:

```sh
./packaging/arch/build-local-package.sh
```

The package installs:

- `/usr/bin/autolon`
- `/usr/share/applications/io.github.autolon.Autolon.desktop`
- `/usr/share/icons/hicolor/scalable/apps/io.github.autolon.Autolon.svg`
- `/usr/share/metainfo/io.github.autolon.Autolon.metainfo.xml`
- `/usr/lib/systemd/user/autolon.service`
- `/usr/lib/udev/rules.d/70-autolon-uinput.rules`
- `/usr/lib/sysusers.d/autolon.conf`

After install:

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
