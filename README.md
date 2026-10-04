# Spacetop

A minimal VR Wayland Compositor and a simple app launcher

## Getting started

Install Rust using [rustup](https://rustup.rs/).

Building input discovery requires `pkg-config` and the libudev development
package (`libudev-dev` on Debian/Ubuntu, or `systemd-devel` on Fedora).

Then start Spacetop from the project directory:

```sh
./start.sh # This mostly runs `cargo run --release` but sets the XDG directories so it uses the launcher app in target/release/spacelauncher
```

## Configuration

Either:

- Edit ~/.config/spacetop/config.toml
- Run `cargo run --release -p spacesettings`

You can also install backgrounds into ~/.config/spacetop/backgrounds/; They are expected to be F32 .exr equirectangular skybox images - you can find some on <https://polyhaven.com/hdris>

## Installation (arch linux)

On Arch Linux, build and install the package from the repository's Arch
packaging directory:

```sh
cd packaging/archlinux
makepkg -si
```

This installs Spacetop, Space Launcher, and Space Settings, including desktop menu entries.
