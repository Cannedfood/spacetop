use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
pub(crate) use spacetop_config::AppConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConfigStamp {
    modified: SystemTime,
    length: u64,
}

fn config_stamp(path: &Path) -> Result<ConfigStamp> {
    let metadata =
        fs::metadata(path).with_context(|| format!("inspect configuration {}", path.display()))?;
    Ok(ConfigStamp {
        modified: metadata.modified()?,
        length: metadata.len(),
    })
}

pub(crate) struct ConfigWatcher {
    path: PathBuf,
    stamp: ConfigStamp,
}

impl ConfigWatcher {
    pub(crate) fn new_default() -> Result<Self> {
        Self::new(AppConfig::path()?)
    }

    fn new(path: PathBuf) -> Result<Self> {
        Ok(Self {
            stamp: config_stamp(&path)?,
            path,
        })
    }

    pub(crate) fn reload_if_changed(&mut self) -> Option<Result<AppConfig>> {
        let stamp = config_stamp(&self.path).ok()?;
        if stamp == self.stamp {
            return None;
        }
        self.stamp = stamp;
        Some(AppConfig::load_from(&self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_floor_parameters() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[floor]\nray_count = 0\n").unwrap();

        assert!(AppConfig::load_from(&path).is_err());
    }

    #[test]
    fn watcher_reloads_changed_file_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[floor]\nray_count = 4\n").unwrap();
        let mut watcher = ConfigWatcher::new(path.clone()).unwrap();

        fs::write(&path, "[floor]\nray_count = 12\n").unwrap();

        let config = watcher.reload_if_changed().unwrap().unwrap();

        assert_eq!(config.floor.ray_count, 12);
        assert!(watcher.reload_if_changed().is_none());
    }
}
