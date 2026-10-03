# Spacetop App Launcher

A small Iced launcher that discovers installed FreeDesktop applications and
starts them with the launcher's inherited environment. In particular, child
apps inherit `WAYLAND_DISPLAY`, so start this launcher from the VR session that
forwards the intended Wayland display.

Build and run from this directory with `cargo run --release`. The window and
application tiles use transparency; transparency support depends on the
Wayland compositor.