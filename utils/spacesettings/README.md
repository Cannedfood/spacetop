# Space Settings

Space Settings provides a small desktop editor for Spacetop's
`~/.config/spacetop/config.toml`. It uses the same shared schema and validation
as the compositor, and saves changes atomically so a running instance can
hot-reload them.

Fullscreen controls configure the maximum window width and height in degrees
and how much the environment is dimmed while a window is fullscreen.
Maximized-window controls set its maximum angular width and height. Maximize
or restore a grabbed window by pressing Y or B while holding grip.

Run from the workspace root:

```sh
cargo run -p spacesettings
```

The included desktop entry is available to SpaceLauncher when started with
`start.sh`. It can also be installed to `~/.local/share/applications/` after
building and installing the `spacesettings` binary on `PATH`.