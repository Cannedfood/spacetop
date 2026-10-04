use std::process::{Child, Command};

use anyhow::Context;

impl spacetop::RuntimeCallbacks for LauncherController {
    fn on_ready(&mut self, displays: spacetop::DisplayNames) -> anyhow::Result<()> {
        self.set_displays(displays);
        Ok(())
    }

    fn on_launcher_toggle(&mut self) -> anyhow::Result<()> {
        self.toggle()
    }
}

pub(crate) struct LauncherController {
    pub(crate) program: String,
    pub(crate) displays: Option<spacetop::DisplayNames>,
    pub(crate) child: Option<Child>,
}

impl LauncherController {
    pub(crate) fn new(program: String) -> Self {
        Self {
            program,
            displays: None,
            child: None,
        }
    }

    pub(crate) fn set_displays(&mut self, displays: spacetop::DisplayNames) {
        self.displays = Some(displays);
    }

    pub(crate) fn toggle(&mut self) -> anyhow::Result<()> {
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
