# Spacetop

A minimal VR Wayland Compositor and a simple app launcher

## Getting started

Install Rust using [rustup](https://rustup.rs/), then start Spacetop from the project directory:

```sh
cargo run --release
```

Install `Xwayland` (version 23.1 or newer) to run X11 applications. Spacetop starts
its own rootless XWayland server automatically when the executable is available.
Without it, native Wayland applications still work.

To launch an app with both display variables set automatically.
Especially useful with app launchers (for example the small [SpaceLauncher](./utils/spacelauncher/README.md) so you can directly start apps on your PC)

```sh
cargo run --release -- --app=./target/release/spacelauncher
```

The compositor prints its Wayland and X11 display names at startup. In another
terminal, launch a native Wayland app on its Wayland display:

```sh
WAYLAND_DISPLAY=<display-name> SOME_APP
```

For an X11 app, use Spacetop's printed X11 display instead of your desktop's display:

```sh
DISPLAY=<x11-display> X11_APP
```

See [TECHNICAL.md](TECHNICAL.md) for runtime requirements, renderer details, and tests.

## Configuration

On first startup Spacetop creates `~/.config/spacetop/config.toml` with defaults.
Changes are picked up automatically while running (polled every 250 ms). Invalid
settings or an unreadable replacement background leave the current settings active;
the default window distance applies to windows opened after the reload.
The background image can be `"random"` to select an EXR from
`~/.config/spacetop/backgrounds/`, or a path to a specific image (including
`~/` paths). The floor height is used when OpenXR does not provide a STAGE floor.

```toml
[background]
image = "random"
brightness_stops = 0.0
rotation_degrees = 0.0

[floor]
height_m = -1.3
albedo = [0.12, 0.12, 0.12, 0.75]
roughness = 0.1
reflectance = 0.18
ray_count = 4
reflection_grain_size_m = 0.002

[window]
default_distance_m = 1.6

[cursor]
default_distance_m = 1.6
```

Background brightness uses exposure stops before tone mapping: `0` preserves the
source, `+1` doubles HDR radiance, and `-1` halves it. Values range from `-8` to
`8` stops. Floor albedo RGBA channels, roughness, and reflectance must be between
`0` and `1`; `ray_count` must be between `1` and `64`; window and cursor
distances must be between `0.1` and `100` meters. Albedo alpha controls floor
opacity. `floor.reflection_grain_size_m` controls the floor-reflection noise
grid spacing; it defaults to `0.002` meters and accepts `0.001` through `0.05`
meters. `background.rotation_degrees` rotates the equirectangular skybox around
the vertical axis from `0` to `360` degrees; `360` is equivalent to `0`.

Edit the configuration with the standalone settings app:

```sh
cargo run -p spacesettings
```

Saving replaces the file atomically; a running Spacetop instance picks up the
new settings automatically.

## Keyboard And Pointer Input

Point with the right controller and pull the trigger to click. Secondary click
uses B on Touch/Index controllers or trackpad click on Vive/Microsoft motion
controllers. The simple-controller profile has no secondary-click binding.
Grip moves the panel; the stick scrolls when not gripping.

To type, choose a keyboard event device and give your user read access to it:

```sh
ls -l /dev/input/by-id/*event-kbd
SPACETOP_INPUT_DEVICES=/dev/input/by-id/YOUR_KEYBOARD-event-kbd cargo run --release -- --app=xterm
```

`YOUR_KEYBOARD` is a placeholder for the device found above. Multiple device
paths can be separated with `:`; mouse devices supply buttons and wheel input
at the controller's current pointer location, not relative mouse movement.
Use `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, and `XKB_DEFAULT_OPTIONS` to
configure typing. Devices are opt-in and are not grabbed exclusively: the host
desktop can receive the same input. Do not run the compositor as root or grant
blanket access to every input device. Automatic seat/device discovery, hotplug,
and an XR virtual keyboard are not implemented.
