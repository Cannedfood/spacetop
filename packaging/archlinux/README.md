# Arch Linux package

The `PKGBUILD` here builds Spacetop and its companion applications from the
upstream Git repository and installs them as system applications:

```sh
cd packaging/archlinux
makepkg -si
```

The build requires `base-devel` and downloads the locked Rust dependencies in
`prepare()`. The package installs `spacetop`, `spacelauncher`, and
`spacesettings` to `/usr/bin`, along with desktop entries for all three
applications. Remove it with `sudo pacman -R spacetop`.

## Runtime requirements

Spacetop is an XR application launched from your existing Linux desktop
session, not a standalone desktop environment or a replacement Wayland login
session. Before launching it:

- Install and start an OpenXR runtime, such as Monado or SteamVR, and ensure
  that the runtime can see the headset.
- Use a Vulkan 1.2-capable GPU and driver with the DMA-BUF sharing capabilities
  described in the project [technical notes](../../TECHNICAL.md#gpu-rendering).
- Ensure the logged-in user has access to the runtime GPU and the input devices
  they want Spacetop to use. The package does not change device permissions.

`xorg-xwayland` is optional; install it to run X11 applications inside Spacetop.
Without it, native Wayland applications can still run. `spacesettings` edits
`~/.config/spacetop/config.toml`; the compositor's default launcher is
`spacelauncher`.

The project currently does not include license metadata. The `unknown` license
value in the PKGBUILD reflects that; check with the project maintainers before
redistributing built packages.
