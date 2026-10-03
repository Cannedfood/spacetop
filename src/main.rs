use std::process::Command;

use anyhow::Context;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|argument| argument == "--xr-client") {
        if args.iter().any(|argument| argument.starts_with("--app=")) {
            return Err("--app cannot be used with --xr-client".into());
        }
        return spacetop::run_xr_client().map_err(Into::into);
    }
    let app = parse_app_argument(&args)?;
    spacetop::run(move |displays| {
        if let Some(app) = app {
            spawn_app(&app, &displays.wayland, displays.x11.as_deref())?;
        }
        Ok(())
    })
    .map_err(Into::into)
}

fn spawn_app(
    app: &str,
    wayland_display: &std::ffi::OsStr,
    x11_display: Option<&str>,
) -> anyhow::Result<()> {
    let mut command = Command::new(app);
    command
        .env("WAYLAND_DISPLAY", wayland_display)
        .env_remove("WAYLAND_SOCKET")
        .env_remove("XAUTHORITY");
    if let Some(display) = x11_display {
        command.env("DISPLAY", display);
    } else {
        command.env_remove("DISPLAY");
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to start app `{app}`"))?;
    eprintln!("Started `{app}` as process {}", child.id());
    Ok(())
}

fn parse_app_argument(args: &[String]) -> Result<Option<String>, String> {
    let mut app = None;
    for argument in args {
        if let Some(value) = argument.strip_prefix("--app=") {
            if value.is_empty() {
                return Err("--app requires an executable name, e.g. --app=totem".into());
            }
            if app.replace(value.to_owned()).is_some() {
                return Err("--app may only be specified once".into());
            }
        } else if argument != "--xr-client" {
            return Err(format!(
                "unknown argument `{argument}`; supported: --app=PROGRAM"
            ));
        }
    }
    Ok(app)
}

#[cfg(test)]
#[path = "../tests/unit/cli.rs"]
mod cli_tests;
