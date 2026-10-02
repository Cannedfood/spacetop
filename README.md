# Spacetop

A Wayland compositor that presents app windows as OpenXR panels on Linux.

## Getting started

Install Rust using [rustup](https://rustup.rs/), then start Spacetop from the project directory:

```sh
cargo run --release
```

The compositor prints its Wayland display name at startup. In another terminal, launch an app on that display:

```sh
WAYLAND_DISPLAY=<display-name> SOME_APP
```

See [TECHNICAL.md](TECHNICAL.md) for runtime requirements, renderer details, and tests.
