use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
pub(crate) use spacetop_config::AppConfig;
#[cfg(test)]
pub(crate) use spacetop_config::DEFAULT_DISTANCE;

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
    fn creates_parent_directory_and_default_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".config/spacetop/config.toml");

        let config = AppConfig::load_from(&path).unwrap();

        assert_eq!(config, AppConfig::default());
        assert_eq!(
            toml::from_str::<AppConfig>(&fs::read_to_string(path).unwrap()).unwrap(),
            config
        );
    }

    #[test]
    fn fills_missing_fields_from_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[floor]\nroughness = 0.4\n").unwrap();

        let config = AppConfig::load_from(&path).unwrap();

        assert_eq!(config.floor.roughness, 0.4);
        assert_eq!(config.floor.albedo, AppConfig::default().floor.albedo);
        assert_eq!(config.background.image, "random");
    }

    #[test]
    fn loads_custom_settings_from_all_sections() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[background]\nimage = \"~/sky.exr\"\nbrightness_stops = 1.5\nrotation_degrees = 135.0\n\
             [floor]\nheight_m = -1.8\nalbedo = [0.2, 0.3, 0.4, 0.9]\n\
             roughness = 0.6\nreflectance = 0.3\nray_count = 12\n\
             reflection_grain_size_m = 0.01\n\
             [window]\ndefault_distance_m = 2.1\n\
             [cursor]\ndefault_distance_m = 1.9\n",
        )
        .unwrap();

        let config = AppConfig::load_from(&path).unwrap();

        assert_eq!(config.background.image, "~/sky.exr");
        assert_eq!(config.background.brightness_stops, 1.5);
        assert_eq!(config.background.rotation_degrees, 135.0);
        assert_eq!(config.floor.height_m, -1.8);
        assert_eq!(config.floor.albedo, [0.2, 0.3, 0.4, 0.9]);
        assert_eq!(config.floor.roughness, 0.6);
        assert_eq!(config.floor.reflectance, 0.3);
        assert_eq!(config.floor.ray_count, 12);
        assert_eq!(config.floor.reflection_grain_size_m, 0.01);
        assert_eq!(config.window.default_distance_m, 2.1);
        assert_eq!(config.cursor.default_distance_m, 1.9);
    }

    #[test]
    fn migrates_legacy_transparency_into_albedo_alpha() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[floor]\nalbedo = [0.2, 0.3, 0.4]\ntransparency = 0.1\n",
        )
        .unwrap();

        let config = AppConfig::load_from(&path).unwrap();
        let saved = toml::to_string(&config).unwrap();

        assert_eq!(config.floor.albedo, [0.2, 0.3, 0.4, 0.9]);
        assert!(!saved.contains("transparency"));
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
        let mut watcher = ConfigWatcher::new(path.clone()).unwrap();

        fs::write(&path, "[floor]\nray_count = 12\n").unwrap();

        let config = watcher.reload_if_changed().unwrap().unwrap();

        assert_eq!(config.floor.ray_count, 12);
        assert!(watcher.reload_if_changed().is_none());
    }
}
