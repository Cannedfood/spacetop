mod launcher;

use spacetop_config::AppConfig;

use crate::launcher::LauncherController;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(std::env::args().skip(1)).map_err(Into::into)
}

fn run(args: impl IntoIterator<Item = String>) -> anyhow::Result<()> {
    let args = args.into_iter().collect::<Vec<_>>();
    let mut config = AppConfig::load()?;
    if let Some(program) = parse_launcher_argument(&args).map_err(anyhow::Error::msg)? {
        config.application.launcher = program;
    }
    let launcher = LauncherController::new(config.application.launcher.clone());
    spacetop::run_with_callbacks(config, launcher)
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
        } else {
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
