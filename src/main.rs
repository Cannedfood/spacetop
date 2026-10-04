use std::{
    process::{Child, Command},
    sync::{Arc, Mutex},
};

use anyhow::{Context, anyhow};
use spacetop_config::AppConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|argument| argument == "--xr-client") {
        if args
            .iter()
            .any(|argument| argument.starts_with("--launcher="))
        {
            return Err("--launcher cannot be used with --xr-client".into());
        }
        return spacetop::run_xr_client().map_err(Into::into);
    }
    let launcher = match parse_launcher_argument(&args)? {
        Some(app) => app,
        None => AppConfig::load()?.application.launcher,
    };
    let launcher = Arc::new(Mutex::new(LauncherController::new(launcher)));
    let ready_launcher = Arc::clone(&launcher);
    spacetop::run(
        move |displays| {
            ready_launcher
                .lock()
                .map_err(|_| anyhow!("launcher controller lock poisoned"))?
                .set_displays(displays);
            Ok(())
        },
        move || {
            launcher
                .lock()
                .map_err(|_| anyhow!("launcher controller lock poisoned"))?
                .toggle()
        },
    )
    .map_err(Into::into)
}

struct LauncherController {
    program: String,
    displays: Option<spacetop::DisplayNames>,
    child: Option<Child>,
}

impl LauncherController {
    fn new(program: String) -> Self {
        Self {
            program,
            displays: None,
            child: None,
        }
    }

    fn set_displays(&mut self, displays: spacetop::DisplayNames) {
        self.displays = Some(displays);
    }

    fn toggle(&mut self) -> anyhow::Result<()> {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait()?.is_none() {
                child
                    .kill()
                    .with_context(|| format!("stop launcher `{}`", self.program))?;
                child
                    .wait()
                    .with_context(|| format!("wait for launcher `{}` to stop", self.program))?;
                self.child = None;
                eprintln!("Stopped launcher `{}`", self.program);
                return Ok(());
            }
            self.child = None;
        }

        let displays = self
            .displays
            .as_ref()
            .context("Wayland display is not ready yet")?;
        let mut command = Command::new(&self.program);
        command
            .env("WAYLAND_DISPLAY", &displays.wayland)
            .env_remove("WAYLAND_SOCKET")
            .env_remove("XAUTHORITY");
        if let Some(display) = &displays.x11 {
            command.env("DISPLAY", display);
        } else {
            command.env_remove("DISPLAY");
        }
        let child = command
            .spawn()
            .with_context(|| format!("failed to start launcher `{}`", self.program))?;
        eprintln!(
            "Started launcher `{}` as process {}",
            self.program,
            child.id()
        );
        self.child = Some(child);
        Ok(())
    }
}

impl Drop for LauncherController {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(error) = child.kill() {
                        eprintln!("Could not stop launcher `{}`: {error}", self.program);
                    } else if let Err(error) = child.wait() {
                        eprintln!("Could not wait for launcher `{}`: {error}", self.program);
                    }
                }
                Err(error) => {
                    eprintln!("Could not check launcher `{}`: {error}", self.program);
                }
            }
        }
    }
}

fn parse_launcher_argument(args: &[String]) -> Result<Option<String>, String> {
    let mut launcher = None;
    for argument in args {
        if let Some(value) = argument.strip_prefix("--launcher=") {
            if value.is_empty() {
                return Err(
                    "--launcher requires an executable name, e.g. --launcher=spacelauncher".into(),
                );
            }
            if launcher.replace(value.to_owned()).is_some() {
                return Err("--launcher may only be specified once".into());
            }
        } else if argument != "--xr-client" {
            return Err(format!(
                "unknown argument `{argument}`; supported: --launcher=PROGRAM"
            ));
        }
    }
    Ok(launcher)
}

#[cfg(test)]
#[path = "../tests/unit/cli.rs"]
mod cli_tests;
