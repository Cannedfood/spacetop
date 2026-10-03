# Spacetop

A Wayland compositor that presents Wayland and X11 app windows as OpenXR panels on Linux.

## Getting started

Install Rust using [rustup](https://rustup.rs/), then start Spacetop from the project directory:

```sh
cargo run --release
```

Install `Xwayland` (version 23.1 or newer) to run X11 applications. Spacetop starts
its own rootless XWayland server automatically when the executable is available.
Without it, native Wayland applications still work.

To launch an app with both display variables set automatically:

```sh
cargo run --release -- --app=xterm
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
