use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
pub(crate) use spacetop_config::{AppConfig, WindowConfig};

pub(crate) struct ConfigWatcher {
    pending_reload: Arc<Mutex<Option<Result<AppConfig>>>>,
    watcher: Option<RecommendedWatcher>,
    worker: Option<JoinHandle<()>>,
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
        let pending_reload = Arc::new(Mutex::new(None));
        let worker_reload = Arc::clone(&pending_reload);
        let worker = thread::Builder::new()
            .name("config-reload".into())
            .spawn(move || {
                let mut deadline: Option<Instant> = None;
                loop {
                    let event = match deadline {
                        Some(deadline_at) => receiver
                            .recv_timeout(deadline_at.saturating_duration_since(Instant::now())),
                        None => receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                    };
                    match event {
                        Ok(event) => {
                            if event.is_err()
                                || event.is_ok_and(|event| {
                                    !matches!(event.kind, notify::EventKind::Access(_))
                                        && event.paths.iter().any(|event_path| event_path == &path)
                                })
                            {
                                deadline = Some(Instant::now() + Duration::from_millis(50));
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let reload = AppConfig::load_from(&path);
                            *worker_reload.lock().expect("config reload lock poisoned") =
                                Some(reload);
                            deadline = None;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self {
            pending_reload,
            watcher: Some(watcher),
            worker: Some(worker),
        })
    }

    pub(crate) fn take_reload(&self) -> Option<Result<AppConfig>> {
        self.pending_reload.try_lock().ok()?.take()
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.watcher.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread};

    fn wait_for_reload(watcher: &ConfigWatcher) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while watcher.pending_reload.lock().unwrap().is_none() {
            assert!(Instant::now() < deadline, "config reload timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }

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
        let watcher = ConfigWatcher::new(path.clone()).unwrap();

        fs::write(&path, "[floor]\nray_count = 12\n").unwrap();
        wait_for_reload(&watcher);
        let config = watcher.take_reload().unwrap().unwrap();

        assert_eq!(config.floor.ray_count, 12);
        thread::sleep(Duration::from_millis(100));
        assert!(watcher.take_reload().is_none());
    }

    #[test]
    fn watcher_loads_without_frame_loop_checks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[floor]\nray_count = 4\n").unwrap();
        let watcher = ConfigWatcher::new(path.clone()).unwrap();

        fs::write(&path, "[floor]\nray_count = 12\n").unwrap();
        wait_for_reload(&watcher);
        fs::remove_file(&path).unwrap();

        let config = watcher.take_reload().unwrap().unwrap();
        assert_eq!(config.floor.ray_count, 12);
    }

    #[test]
    fn watcher_recovers_from_invalid_config_with_atomic_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[floor]\nray_count = 4\n").unwrap();
        let watcher = ConfigWatcher::new(path.clone()).unwrap();

        fs::write(&path, "[floor]\nray_count = 0\n").unwrap();
        wait_for_reload(&watcher);
        assert!(watcher.take_reload().unwrap().is_err());

        let replacement = directory.path().join("replacement.toml");
        fs::write(&replacement, "[floor]\nray_count = 12\n").unwrap();
        fs::rename(&replacement, &path).unwrap();
        wait_for_reload(&watcher);
        assert_eq!(watcher.take_reload().unwrap().unwrap().floor.ray_count, 12);

        fs::write(directory.path().join("unrelated.toml"), "invalid").unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(watcher.take_reload().is_none());
    }
}
