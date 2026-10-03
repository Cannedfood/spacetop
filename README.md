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
