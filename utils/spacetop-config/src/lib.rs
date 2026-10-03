use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

pub const DEFAULT_DISTANCE: f32 = 1.6;
pub const FALLBACK_FLOOR_HEIGHT: f32 = -1.3;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub background: BackgroundConfig,
    pub floor: FloorConfig,
    pub window: DistanceConfig,
    pub cursor: DistanceConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct BackgroundConfig {
    pub image: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct FloorConfig {
    pub height_m: f32,
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub reflectance: f32,
    pub transparency: f32,
    pub ray_count: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct DistanceConfig {
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
            height_m: FALLBACK_FLOOR_HEIGHT,
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
    pub fn path() -> Result<PathBuf> {
        Ok(std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set; cannot locate Spacetop configuration")?
            .join(".config/spacetop/config.toml"))
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let parent = path.parent().context("configuration path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create configuration directory {}", parent.display()))?;
        if !path.exists() {
            let defaults = toml::to_string_pretty(&Self::default())?;
            let mut file = NamedTempFile::new_in(parent)
                .with_context(|| format!("create default config beside {}", path.display()))?;
            file.write_all(defaults.as_bytes())?;
            file.as_file().sync_all()?;
            match file.persist_noclobber(path) {
                Ok(_) => eprintln!("Created configuration: {}", path.display()),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error.error)
                        .with_context(|| format!("create configuration {}", path.display()));
                }
            }
        }
        let contents = fs::read_to_string(path)
            .with_context(|| format!("read configuration {}", path.display()))?;
        let config: Self = toml::from_str(&contents)
            .with_context(|| format!("parse configuration {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
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

    pub fn save_to(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().context("configuration path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create configuration directory {}", parent.display()))?;
        let serialized = toml::to_string_pretty(self)?;
        let mut file = NamedTempFile::new_in(parent)
            .with_context(|| format!("create temporary config beside {}", path.display()))?;
        file.write_all(serialized.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("save configuration {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_missing_config_and_round_trips_defaults() {
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
    fn save_rejects_invalid_values_without_replacing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = toml::to_string(&AppConfig::default()).unwrap();
        fs::write(&path, &original).unwrap();
        let mut invalid = AppConfig::default();
        invalid.floor.ray_count = 0;

        assert!(invalid.save_to(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }
}
