# Space Settings

Space Settings provides a small desktop editor for Spacetop's
`~/.config/spacetop/config.toml`. It uses the same shared schema and validation
as the compositor, and saves changes atomically so a running instance can
hot-reload them.

Run from the workspace root:

```sh
cargo run -p spacesettings
```

The included desktop entry can be installed to
`~/.local/share/applications/` after building and installing the `spacesettings`
binary on `PATH`.