# Spacetop

A minimal VR Wayland Compositor and a simple app launcher

## Getting started

Install Rust using [rustup](https://rustup.rs/).

Building input discovery requires `pkg-config` and the libudev development
package (`libudev-dev` on Debian/Ubuntu, or `systemd-devel` on Fedora).

Then start Spacetop from the project directory:

```sh
cargo run --release
```

### Arch Linux installation

On Arch Linux, build and install the package from the repository's Arch
packaging directory:

```sh
cd packaging/archlinux
makepkg -si
```

This installs Spacetop, Space Launcher, and Space Settings, including desktop
menu entries. Spacetop runs as an application inside your existing desktop
session; it requires a separately installed and configured OpenXR runtime and a
compatible Vulkan 1.2 GPU. See
[the Arch Linux packaging notes](packaging/archlinux/README.md) for
dependencies and runtime setup.

Install `Xwayland` (version 23.1 or newer) to run X11 applications. Spacetop starts
its own rootless XWayland server automatically when the executable is available.
Without it, native Wayland applications still work.

Press the controller's B button to open or close the configured launcher (by
default [SpaceLauncher](./utils/spacelauncher/README.md)). The launcher starts
closed. Use `--launcher` to override it for a session:

```sh
cargo run --release -- --launcher=./target/release/spacelauncher
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
`~/` paths). Skyboxes load asynchronously, use RGBA16F textures, and fade out
while a replacement loads. Invalid HDR channels are repaired or clamped before
upload. The floor height is used when OpenXR does not provide a STAGE floor.

```toml
config_version = 1

[application]
launcher = "spacelauncher"

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
trace_through_transparent_windows = false
ambient_occlusion = false
radius_degrees = 90.0
feathering_m = 0.0

[window]
default_distance_m = 1.6
pixels_per_degree = 30.0
display_scale = 1.0
reflection_atlas_size = 256
fullscreen_max_width_degrees = 100.0
fullscreen_max_height_degrees = 75.0
fullscreen_environment_dim = 0.5
maximized_max_width_degrees = 70.0
maximized_max_height_degrees = 50.0

[cursor]
default_distance_m = 1.6
```

`window.pixels_per_degree` is the density in logical pixels per degree.
`window.display_scale` (0.5–4.0 in 0.5 increments, default 1.0) is advertised
to Wayland clients; lower values reduce capture resolution, while higher values
let clients render larger buffers without changing spatial density or panel size.

Floor reflections use a texture atlas. `window.reflection_atlas_size` sets its
width and height in pixels (default: 256 x 256, approximately 256 KiB). Options
are 256, 512, 1024, 2048, 4096, and 8192. Size changes apply while running.
Window textures are scaled down proportionally
when necessary to fit every window, without changing ordinary window rendering.
Sizes exceeding the GPU's image limit are rejected.

Background brightness uses exposure stops before tone mapping: `0` preserves the
source, `+1` doubles HDR radiance, and `-1` halves it. Values range from `-8` to
`8` stops. Floor albedo RGBA channels, roughness, and reflectance must be between
`0` and `1`; `ray_count` must be between `1` and `64`; window and cursor
distances must be between `0.1` and `100` meters. Albedo alpha controls floor
opacity. `floor.reflection_grain_size_m` controls the floor-reflection noise
grid spacing; it defaults to `0.002` meters and accepts `0.001` through `0.05`
meters. `floor.trace_through_transparent_windows` defaults to `false`, so each reflection
ray samples only its nearest window hit (or the skybox on a miss). Set it to
`true` to continue through transparent window texels and composite farther hits.
The renderer uses separate precompiled shader variants, so the setting does not
add a runtime shader branch to the default first-hit path.
`floor.ambient_occlusion` defaults to `false` and can be enabled from the
experimental ambient-occlusion setting in SpaceSettings. When enabled, windows
reduce the floor's diffuse sky illumination according to their coverage of the
sky.
`floor.radius_degrees` sets the ground's outer radius as an angle from straight
down (`-Y`), between 0 and 90 degrees. `floor.feathering_m` sets how far inward
from that edge the ground smoothly fades into the skybox, between 0 and 7 meters.
The default 90-degree radius extends the ground to the horizon; smaller angles
limit it to a circle around the LOCAL-space origin. The angular radius scales with
the origin-to-floor height, while the feathering width remains a fixed ground
distance.
`background.rotation_degrees` rotates the equirectangular skybox around
the vertical axis from `0` to `360` degrees; `360` is equivalent to `0`.

Edit the configuration with the standalone settings app:

```sh
cargo run -p spacesettings
```

Saving replaces the file atomically; a running Spacetop instance picks up the
new settings automatically.

Wayland and X11 windows can request fullscreen. The fullscreen window is placed
in front of the user in LOCAL space, fitted within the configured maximum
angular width and height, while the other windows are hidden and the
environment is dimmed. The fullscreen width and height settings accept
`1`–`170` degrees; environment dimming accepts `0` (off) to `1` (fully dimmed).
Space Settings exposes these controls in its Fullscreen section. When the
window leaves fullscreen, the previous window layout is restored.

Maximize a grabbed window by pressing Y or B while holding grip. A maximized
window is fitted within its configured maximum angular dimensions (defaults:
70° wide by 50° high) and otherwise behaves like a normal window: other
windows remain visible, collisions are resolved, and the environment is not
dimmed. Press Y or B again while still grabbing it to restore its prior size.

## Keyboard And Pointer Input

Point with the right controller and pull the trigger, or press X/A on the
Touch controllers (A on Index controllers), to left-click. Pressing the right
thumbstick clicks the middle mouse button on controllers with a thumbstick;
while grabbing a window, pressing it closes the grabbed window instead.
Press B on Touch/Index controllers or trackpad click on Vive/Microsoft motion
controllers to open or close the configured launcher. Grip moves the panel;
the stick scrolls when not gripping.

Spacetop automatically discovers readable keyboard and mouse event devices on
the current seat (`XDG_SEAT`, or `seat0` if unset), using udev's keyboard/mouse
classification. It rescans every second for newly connected devices,
reconnection, and permission changes. Disconnecting a device releases its held
keys and buttons. Input-device read permissions must still be configured
externally; discovery does not grant access.

To select specific devices instead of automatic discovery, set
`SPACETOP_INPUT_DEVICES`:

```sh
ls -l /dev/input/by-id/*event-kbd
SPACETOP_INPUT_DEVICES=/dev/input/by-id/YOUR_KEYBOARD-event-kbd cargo run --release -- --launcher=xterm
```

`YOUR_KEYBOARD` is a placeholder for the device found above. Explicitly selected
devices must be accessible at startup; after startup they are retried if
disconnected. Prefer stable `/dev/input/by-id/` paths for reconnection. Set
`SPACETOP_INPUT_DEVICES` to an empty string to disable physical input.
Multiple device paths can be separated with `:`; mouse devices supply buttons and wheel input
and relative movement. Moving the mouse takes over the XR pointer at the
headset's view center; subsequent mouse movement steers it. Noticeable
controller movement takes control back. The mouse-controlled cursor hides after
two seconds without mouse movement. Buttons and wheel input use the active
pointer location.
Use `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, and `XKB_DEFAULT_OPTIONS` to
configure typing. Devices are not grabbed exclusively: the host desktop can
receive the same input, even while interacting with XR windows. Automatic
discovery excludes devices not classified as keyboards or mice, such as
gamepads and touchpads. Do not run the compositor as root or grant blanket
access to every input device. An XR virtual keyboard and relative-pointer
protocol support are not implemented.
