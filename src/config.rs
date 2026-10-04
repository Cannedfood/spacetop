use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
pub(crate) use spacetop_config::{AppConfig, WindowConfig};

pub(crate) struct ConfigWatcher {
    path: PathBuf,
    receiver: mpsc::Receiver<notify::Result<notify::Event>>,
    _watcher: RecommendedWatcher,
    last_event_at: Option<Instant>,
}

impl ConfigWatcher {
    pub(crate) fn new_default() -> Result<Self> {
        Self::new(AppConfig::path()?)
    }

    fn new(path: PathBuf) -> Result<Self> {
        let directory = path
            .parent()
            .context("configuration path has no parent directory")?;
        let (sender, receiver) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(sender, notify::Config::default())?;
        watcher.watch(directory, RecursiveMode::NonRecursive)?;
        Ok(Self {
            path,
            receiver,
            _watcher: watcher,
            last_event_at: None,
        })
    }

    pub(crate) fn reload_if_changed(&mut self) -> Option<Result<AppConfig>> {
        let now = Instant::now();
        let mut changed = false;
        for event in self.receiver.try_iter() {
            if event.is_err()
                || event.is_ok_and(|event| {
                    event
                        .paths
                        .iter()
                        .any(|path| path.file_name() == self.path.file_name())
                })
            {
                changed = true;
            }
        }
        if changed {
            self.last_event_at = Some(now);
        }
        let last_event_at = self.last_event_at?;
        if now.duration_since(last_event_at) < Duration::from_millis(50) {
            return None;
        }
        self.last_event_at = None;
        Some(AppConfig::load_from(&self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread};

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

        let config = (0..100)
            .find_map(|_| {
                let config = watcher.reload_if_changed();
                if config.is_none() {
                    thread::sleep(Duration::from_millis(10));
                }
                config
            })
            .unwrap()
            .unwrap();

        assert_eq!(config.floor.ray_count, 12);
        assert!(watcher.reload_if_changed().is_none());
    }
}
