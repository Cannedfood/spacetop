use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_DISTANCE: f32 = 1.6;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct AppConfig {
    pub background: BackgroundConfig,
    pub floor: FloorConfig,
    pub window: DistanceConfig,
    pub cursor: DistanceConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct BackgroundConfig {
    pub image: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct FloorConfig {
    pub height_m: f32,
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub reflectance: f32,
    pub transparency: f32,
    pub ray_count: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct DistanceConfig {
    pub default_distance_m: f32,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            image: "random".into(),
        }
    }
}

impl Default for FloorConfig {
    fn default() -> Self {
        Self {
            height_m: crate::scene::FALLBACK_FLOOR_Y,
            albedo: [0.12, 0.12, 0.12],
            roughness: 0.1,
            reflectance: 0.18,
            transparency: 0.25,
            ray_count: 4,
        }
    }
}

impl Default for DistanceConfig {
    fn default() -> Self {
        Self {
            default_distance_m: DEFAULT_DISTANCE,
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path()?)
    }

    pub(crate) fn path() -> Result<PathBuf> {
        Ok(std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set; cannot locate Spacetop configuration")?
            .join(".config/spacetop/config.toml"))
    }

    fn load_from(path: &Path) -> Result<Self> {
        let parent = path.parent().context("configuration path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create configuration directory {}", parent.display()))?;
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                let defaults = toml::to_string_pretty(&Self::default())?;
                file.write_all(defaults.as_bytes())
                    .with_context(|| format!("write default configuration {}", path.display()))?;
                eprintln!("Created configuration: {}", path.display());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create configuration {}", path.display()));
            }
        }
        let contents = fs::read_to_string(path)
            .with_context(|| format!("read configuration {}", path.display()))?;
        let config: Self = toml::from_str(&contents)
            .with_context(|| format!("parse configuration {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.background.image == "random" || !self.background.image.is_empty(),
            "background.image must be `random` or a non-empty file path"
        );
        ensure!(
            self.floor.height_m.is_finite(),
            "floor.height_m must be finite"
        );
        ensure!(
            self.floor
                .albedo
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value)),
            "floor.albedo channels must be finite and between 0 and 1"
        );
        ensure!(
            self.floor.roughness.is_finite() && (0.0..=1.0).contains(&self.floor.roughness),
            "floor.roughness must be between 0 and 1"
        );
        ensure!(
            self.floor.reflectance.is_finite() && (0.0..=1.0).contains(&self.floor.reflectance),
            "floor.reflectance must be between 0 and 1"
        );
        ensure!(
            self.floor.transparency.is_finite() && (0.0..=1.0).contains(&self.floor.transparency),
            "floor.transparency must be between 0 and 1"
        );
        ensure!(
            (1..=64).contains(&self.floor.ray_count),
            "floor.ray_count must be between 1 and 64"
        );
        for (name, distance) in [
            ("window.default_distance_m", self.window.default_distance_m),
            ("cursor.default_distance_m", self.cursor.default_distance_m),
        ] {
            ensure!(
                distance.is_finite() && (0.1..=100.0).contains(&distance),
                "{name} must be between 0.1 and 100 meters"
            );
        }
        Ok(())
    }
}

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
            "[background]\nimage = \"~/sky.exr\"\n\
             [floor]\nheight_m = -1.8\nalbedo = [0.2, 0.3, 0.4]\n\
             roughness = 0.6\nreflectance = 0.3\ntransparency = 0.1\nray_count = 12\n\
             [window]\ndefault_distance_m = 2.1\n\
             [cursor]\ndefault_distance_m = 1.9\n",
        )
        .unwrap();

        let config = AppConfig::load_from(&path).unwrap();

        assert_eq!(config.background.image, "~/sky.exr");
        assert_eq!(config.floor.height_m, -1.8);
        assert_eq!(config.floor.albedo, [0.2, 0.3, 0.4]);
        assert_eq!(config.floor.roughness, 0.6);
        assert_eq!(config.floor.reflectance, 0.3);
        assert_eq!(config.floor.transparency, 0.1);
        assert_eq!(config.floor.ray_count, 12);
        assert_eq!(config.window.default_distance_m, 2.1);
        assert_eq!(config.cursor.default_distance_m, 1.9);
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
