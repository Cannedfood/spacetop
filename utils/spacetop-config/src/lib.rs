use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

mod migration;

use migration::migrate_config;

const CURRENT_CONFIG_VERSION: i64 = 1;
const CONFIG_VERSION_KEY: &str = "config_version";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub config_version: i64,
    pub background: BackgroundConfig,
    pub application: ApplicationConfig,
    pub floor: FloorConfig,
    pub window: WindowConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ApplicationConfig {
    pub launcher: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct BackgroundConfig {
    pub image: String,
    pub brightness_stops: f32,
    pub rotation_degrees: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct FloorConfig {
    pub height_m: f32,
    pub albedo: [f32; 4],
    pub roughness: f32,
    pub reflectance: f32,
    pub ray_count: u32,
    pub trace_through_transparent_windows: bool,
    pub ambient_occlusion: bool,
    pub reflection_grain_size_m: f32,
    pub radius_degrees: f32,
    pub feathering_m: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    pub pixels_per_degree: f32,
    pub display_scale: f32,
    pub texture_aa: WindowTextureAa,
    pub reflection_atlas_size: ReflectionAtlasSize,

    pub default_distance_m: f32,
    pub default_vertical_angle_degrees: f32,
    pub animation_half_time_s: f32,
    pub collision_margin_m: f32,
    pub fullscreen_max_width_degrees: f32,
    pub fullscreen_max_height_degrees: f32,
    pub fullscreen_environment_dim: f32,
    pub maximized_max_width_degrees: f32,
    pub maximized_max_height_degrees: f32,

    pub cursor_proximity_radius_px: f32,
    pub padding_px: f32,
    pub margin_px: f32,
    pub border_radius_px: f32,
    pub border_width_px: f32,
    pub cursor_close_border_width_px: f32,
    pub grabbed_border_width_px: f32,

    pub border_color: [f32; 4],
    pub cursor_close_border_color: [f32; 4],
    pub grabbed_border_color: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "u32", into = "u32")]
#[repr(u32)]
pub enum ReflectionAtlasSize {
    #[default]
    Size256 = 256,
    Size512 = 512,
    Size1024 = 1024,
    Size2048 = 2048,
    Size4096 = 4096,
    Size8192 = 8192,
}

impl ReflectionAtlasSize {
    pub const OPTIONS: [Self; 6] = [
        Self::Size256,
        Self::Size512,
        Self::Size1024,
        Self::Size2048,
        Self::Size4096,
        Self::Size8192,
    ];
}

impl From<ReflectionAtlasSize> for u32 {
    fn from(size: ReflectionAtlasSize) -> Self {
        size as u32
    }
}

impl TryFrom<u32> for ReflectionAtlasSize {
    type Error = anyhow::Error;

    fn try_from(size: u32) -> Result<Self> {
        Self::OPTIONS
            .into_iter()
            .find(|option| u32::from(*option) == size)
            .context("window.reflection_atlas_size must be 256, 512, 1024, 2048, 4096, or 8192")
    }
}

impl std::fmt::Display for ReflectionAtlasSize {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let size = u32::from(*self);
        write!(formatter, "{size} x {size}")
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowTextureAa {
    Nearest,
    #[default]
    SuperSample2x2,
    SuperSample4,
    SuperSample4x2,
    SuperSample8,
    SuperSample8x2,
    SuperSample16,
}

impl WindowTextureAa {
    pub const OPTIONS: [Self; 7] = [
        Self::Nearest,
        Self::SuperSample2x2,
        Self::SuperSample4,
        Self::SuperSample4x2,
        Self::SuperSample8,
        Self::SuperSample8x2,
        Self::SuperSample16,
    ];

    pub fn samples_per_frame(self) -> u32 {
        match self {
            Self::Nearest => 1,
            Self::SuperSample2x2 | Self::SuperSample4 => 2,
            Self::SuperSample4x2 | Self::SuperSample8 => 4,
            Self::SuperSample8x2 | Self::SuperSample16 => 8,
        }
    }

    pub fn pattern_samples(self) -> u32 {
        match self {
            Self::Nearest => 1,
            Self::SuperSample2x2 | Self::SuperSample4 => 4,
            Self::SuperSample4x2 | Self::SuperSample8 => 8,
            Self::SuperSample8x2 | Self::SuperSample16 => 16,
        }
    }

    pub fn is_temporal(self) -> bool {
        matches!(
            self,
            Self::SuperSample2x2 | Self::SuperSample4x2 | Self::SuperSample8x2
        )
    }

    pub fn shader_mode(self) -> u32 {
        match self {
            Self::Nearest => 0,
            Self::SuperSample2x2 => 1,
            Self::SuperSample4 => 2,
            Self::SuperSample4x2 => 3,
            Self::SuperSample8 => 4,
            Self::SuperSample8x2 => 5,
            Self::SuperSample16 => 6,
        }
    }
}

impl std::fmt::Display for WindowTextureAa {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Nearest => "1",
            Self::SuperSample2x2 => "2x2",
            Self::SuperSample4 => "4",
            Self::SuperSample4x2 => "4x2",
            Self::SuperSample8 => "8",
            Self::SuperSample8x2 => "8x2",
            Self::SuperSample16 => "16",
        })
    }
}

impl WindowConfig {
    pub fn max_border_width_px(&self) -> f32 {
        self.border_width_px
            .max(self.cursor_close_border_width_px)
            .max(self.grabbed_border_width_px)
    }

    pub fn effective_padding_px(&self) -> f32 {
        self.padding_px.max(self.max_border_width_px() * 0.5)
    }

    pub fn effective_margin_px(&self) -> f32 {
        self.margin_px.max(self.max_border_width_px() * 0.5)
    }

    pub fn grab_reach_px(&self) -> f32 {
        self.max_border_width_px()
            .max(self.padding_px + self.margin_px)
    }
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self {
            launcher: "spacelauncher".into(),
        }
    }
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            image: "random".into(),
            brightness_stops: -0.1,
            rotation_degrees: 0.0,
        }
    }
}

impl Default for FloorConfig {
    fn default() -> Self {
        Self {
            height_m: -1.3,
            albedo: [0.5367573, 0.5114081, 0.5114081, 1.0],
            roughness: 0.06,
            reflectance: 0.15,
            ray_count: 1,
            trace_through_transparent_windows: true,
            ambient_occlusion: false,
            reflection_grain_size_m: 0.001,
            radius_degrees: 90.0,
            feathering_m: 0.0,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            pixels_per_degree: 30.0,
            display_scale: 1.0,
            texture_aa: WindowTextureAa::default(),
            reflection_atlas_size: ReflectionAtlasSize::default(),

            default_distance_m: 1.6,
            default_vertical_angle_degrees: -10.0,
            animation_half_time_s: 0.2,
            collision_margin_m: 0.04,
            fullscreen_max_width_degrees: 100.0,
            fullscreen_max_height_degrees: 75.0,
            fullscreen_environment_dim: 0.5,
            maximized_max_width_degrees: 70.0,
            maximized_max_height_degrees: 50.0,

            cursor_proximity_radius_px: 301.0,
            padding_px: 42.0,
            margin_px: 4.0,
            border_radius_px: 42.0,
            border_width_px: 0.0,
            cursor_close_border_width_px: 2.0,
            grabbed_border_width_px: 1.0,

            border_color: [0.9911504, 0.9911504, 0.9911504, 0.0],
            cursor_close_border_color: [0.2485206, 0.9534924, 0.8007485, 0.5680294],
            grabbed_border_color: [0.34, 0.82, 0.72, 1.0],
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            background: BackgroundConfig::default(),
            application: ApplicationConfig::default(),
            floor: FloorConfig::default(),
            window: WindowConfig::default(),
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
        let path = Self::path()?;
        Self::create_default_at(&path)?;
        Self::load_from(&path)
    }

    pub fn create_default_at(path: &Path) -> Result<()> {
        let parent = path.parent().context("configuration path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create configuration directory {}", parent.display()))?;
        if path.exists() {
            return Ok(());
        }

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

        Ok(())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("read configuration {}", path.display()))?;
        let mut value: toml::Value = toml::from_str(&contents)
            .with_context(|| format!("parse configuration {}", path.display()))?;
        migrate_config(&mut value)
            .with_context(|| format!("migrate configuration {}", path.display()))?;
        let config: Self = value
            .try_into()
            .with_context(|| format!("parse configuration {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.config_version == CURRENT_CONFIG_VERSION,
            "config_version must be {CURRENT_CONFIG_VERSION}"
        );
        ensure!(
            !self.application.launcher.trim().is_empty(),
            "application.launcher must be a non-empty executable name"
        );
        ensure!(
            self.background.image == "random" || !self.background.image.is_empty(),
            "background.image must be `random` or a non-empty file path"
        );
        ensure!(
            self.background.brightness_stops.is_finite()
                && (-8.0..=8.0).contains(&self.background.brightness_stops),
            "background.brightness_stops must be between -8 and 8 stops"
        );
        ensure!(
            self.background.rotation_degrees.is_finite()
                && (0.0..=360.0).contains(&self.background.rotation_degrees),
            "background.rotation_degrees must be between 0 and 360 degrees"
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
            "floor.albedo RGBA channels must be finite and between 0 and 1"
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
            (1..=64).contains(&self.floor.ray_count),
            "floor.ray_count must be between 1 and 64"
        );
        ensure!(
            self.floor.reflection_grain_size_m.is_finite()
                && (0.001..=0.05).contains(&self.floor.reflection_grain_size_m),
            "floor.reflection_grain_size_m must be between 0.001 and 0.05 meters"
        );
        ensure!(
            self.floor.radius_degrees.is_finite()
                && (0.0..=90.0).contains(&self.floor.radius_degrees),
            "floor.radius_degrees must be between 0 and 90 degrees"
        );
        ensure!(
            self.floor.feathering_m.is_finite() && (0.0..=7.0).contains(&self.floor.feathering_m),
            "floor.feathering_m must be between 0 and 7 meters"
        );
        ensure!(
            self.window.default_distance_m.is_finite()
                && (0.1..=100.0).contains(&self.window.default_distance_m),
            "window.default_distance_m must be between 0.1 and 100 meters"
        );
        ensure!(
            self.window.default_vertical_angle_degrees.is_finite()
                && (-90.0..=90.0).contains(&self.window.default_vertical_angle_degrees),
            "window.default_vertical_angle_degrees must be between -90 and 90 degrees"
        );
        ensure!(
            self.window.pixels_per_degree.is_finite()
                && (1.0..=200.0).contains(&self.window.pixels_per_degree),
            "window.pixels_per_degree must be between 1 and 200"
        );
        ensure!(
            self.window.display_scale.is_finite()
                && (0.5..=4.0).contains(&self.window.display_scale)
                && (self.window.display_scale * 2.0).fract() == 0.0,
            "window.display_scale must be between 0.5 and 4 in 0.5 increments"
        );
        ensure!(
            self.window.animation_half_time_s.is_finite()
                && (0.0..=1.0).contains(&self.window.animation_half_time_s),
            "window.animation_half_time_s must be between 0 and 1 second"
        );
        ensure!(
            self.window.collision_margin_m.is_finite()
                && (0.0..=0.5).contains(&self.window.collision_margin_m),
            "window.collision_margin_m must be between 0 and 0.5 meters"
        );
        ensure!(
            self.window.fullscreen_max_width_degrees.is_finite()
                && (1.0..=170.0).contains(&self.window.fullscreen_max_width_degrees),
            "window.fullscreen_max_width_degrees must be between 1 and 170 degrees"
        );
        ensure!(
            self.window.fullscreen_max_height_degrees.is_finite()
                && (1.0..=170.0).contains(&self.window.fullscreen_max_height_degrees),
            "window.fullscreen_max_height_degrees must be between 1 and 170 degrees"
        );
        ensure!(
            self.window.fullscreen_environment_dim.is_finite()
                && (0.0..=1.0).contains(&self.window.fullscreen_environment_dim),
            "window.fullscreen_environment_dim must be between 0 and 1"
        );
        ensure!(
            self.window.maximized_max_width_degrees.is_finite()
                && (1.0..=170.0).contains(&self.window.maximized_max_width_degrees),
            "window.maximized_max_width_degrees must be between 1 and 170 degrees"
        );
        ensure!(
            self.window.maximized_max_height_degrees.is_finite()
                && (1.0..=170.0).contains(&self.window.maximized_max_height_degrees),
            "window.maximized_max_height_degrees must be between 1 and 170 degrees"
        );
        for (name, value, maximum) in [
            ("window.padding_px", self.window.padding_px, 500.0),
            ("window.margin_px", self.window.margin_px, 500.0),
            ("window.border_width_px", self.window.border_width_px, 100.0),
            (
                "window.cursor_proximity_radius_px",
                self.window.cursor_proximity_radius_px,
                500.0,
            ),
            (
                "window.cursor_close_border_width_px",
                self.window.cursor_close_border_width_px,
                100.0,
            ),
            (
                "window.grabbed_border_width_px",
                self.window.grabbed_border_width_px,
                100.0,
            ),
            (
                "window.border_radius_px",
                self.window.border_radius_px,
                500.0,
            ),
        ] {
            ensure!(
                value.is_finite() && (0.0..=maximum).contains(&value),
                "{name} must be between 0 and {maximum} pixels"
            );
        }
        for (name, color) in [
            ("window.border_color", self.window.border_color),
            (
                "window.cursor_close_border_color",
                self.window.cursor_close_border_color,
            ),
            (
                "window.grabbed_border_color",
                self.window.grabbed_border_color,
            ),
        ] {
            ensure!(
                color
                    .iter()
                    .all(|channel| channel.is_finite() && (0.0..=1.0).contains(channel)),
                "{name} RGBA channels must be finite and between 0 and 1"
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
    fn reflection_atlas_sizes_accept_supported_values_and_reject_invalid_values() {
        for size in ReflectionAtlasSize::OPTIONS {
            let config: AppConfig = toml::from_str(&format!(
                "[window]\nreflection_atlas_size = {}\n",
                u32::from(size)
            ))
            .unwrap();
            assert_eq!(config.window.reflection_atlas_size, size);
        }
        for size in ["0", "1000", "-1", "\"1024\""] {
            assert!(
                toml::from_str::<AppConfig>(&format!("[window]\nreflection_atlas_size = {size}\n"))
                    .is_err()
            );
        }
    }

    #[test]
    fn creates_default_config_with_current_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".config/spacetop/config.toml");

        AppConfig::create_default_at(&path).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        let value: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(
            value
                .get(CONFIG_VERSION_KEY)
                .and_then(toml::Value::as_integer),
            Some(1)
        );
    }

    #[test]
    fn load_from_does_not_create_missing_config_or_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing/config.toml");

        assert!(AppConfig::load_from(&path).is_err());
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn migrates_unversioned_legacy_fields_before_deserializing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "[floor]\nalbedo = [0.2, 0.3, 0.4]\ntransparency = 0.1\n\
             [window]\ntexture_samples = 8\ntemporal_texture_aa = false\n",
        )
        .unwrap();

        let config = AppConfig::load_from(&path).unwrap();

        assert_eq!(config.floor.albedo, [0.2, 0.3, 0.4, 0.9]);
        assert_eq!(config.window.texture_aa, WindowTextureAa::SuperSample8);
        assert_eq!(config.window.display_scale, 1.0);

        config.save_to(&path).unwrap();
        let saved: toml::Value = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            saved
                .get(CONFIG_VERSION_KEY)
                .and_then(toml::Value::as_integer),
            Some(1)
        );
        assert!(saved["floor"].get("transparency").is_none());
        assert!(saved["window"].get("texture_samples").is_none());
        assert!(saved["window"].get("temporal_texture_aa").is_none());
        assert_eq!(
            saved["window"]["texture_aa"].as_str(),
            Some("super_sample8")
        );

        let mut alias: toml::Value =
            toml::from_str("[window]\ntexture_aa = \"four_by_two\"\n").unwrap();
        migrate_config(&mut alias).unwrap();
        let migrated: AppConfig = alias.try_into().unwrap();
        assert_eq!(migrated.window.texture_aa, WindowTextureAa::SuperSample4x2);
    }

    #[test]
    fn rejects_newer_configuration_versions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "config_version = 2\n").unwrap();

        let error = AppConfig::load_from(&path).unwrap_err();

        assert!(format!("{error:#}").contains("newer than supported version"));
    }

    #[test]
    fn rejects_non_current_config_version_when_saving() {
        let config = AppConfig {
            config_version: 0,
            ..AppConfig::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_an_empty_launcher_executable() {
        let mut config = AppConfig::default();
        config.application.launcher = "  ".into();

        assert!(config.validate().is_err());
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

    #[test]
    fn validates_background_brightness_stops() {
        let mut config = AppConfig::default();
        config.background.brightness_stops = 1.5;
        assert!(config.validate().is_ok());

        config.background.brightness_stops = -8.0;
        assert!(config.validate().is_ok());

        config.background.brightness_stops = 8.0;
        assert!(config.validate().is_ok());

        config.background.brightness_stops = -8.1;
        assert!(config.validate().is_err());

        config.background.brightness_stops = 8.1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_background_rotation_degrees() {
        let mut config = AppConfig::default();
        config.background.rotation_degrees = 360.0;
        assert!(config.validate().is_ok());

        config.background.rotation_degrees = -0.1;
        assert!(config.validate().is_err());

        config.background.rotation_degrees = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_floor_reflection_grain_size() {
        let mut config = AppConfig::default();
        config.floor.reflection_grain_size_m = 0.02;
        assert!(config.validate().is_ok());

        config.floor.reflection_grain_size_m = 0.0009;
        assert!(config.validate().is_err());

        config.floor.reflection_grain_size_m = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_floor_radius_angles() {
        let mut config = AppConfig::default();
        config.floor.radius_degrees = 60.0;
        assert!(config.validate().is_ok());

        config.floor.radius_degrees = -1.0;
        assert!(config.validate().is_err());

        config.floor.radius_degrees = f32::NAN;
        assert!(config.validate().is_err());

        config.floor.radius_degrees = 60.0;
        config.floor.feathering_m = 7.0;
        assert!(config.validate().is_ok());

        config.floor.feathering_m = 7.1;
        assert!(config.validate().is_err());

        config.floor.feathering_m = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_window_pixels_per_degree() {
        let mut config = AppConfig::default();
        config.window.pixels_per_degree = 64.0;
        assert!(config.validate().is_ok());

        config.window.pixels_per_degree = 0.0;
        assert!(config.validate().is_err());

        config.window.pixels_per_degree = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_window_display_scale() {
        let mut config = AppConfig::default();
        for scale in [0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0] {
            config.window.display_scale = scale;
            assert!(config.validate().is_ok(), "{scale}");
        }

        config.window.display_scale = 0.75;
        assert!(config.validate().is_err());

        config.window.display_scale = 4.5;
        assert!(config.validate().is_err());

        config.window.display_scale = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_window_default_vertical_angle() {
        let mut config = AppConfig::default();
        config.window.default_vertical_angle_degrees = -90.0;
        assert!(config.validate().is_ok());

        config.window.default_vertical_angle_degrees = 30.0;
        assert!(config.validate().is_ok());

        config.window.default_vertical_angle_degrees = 90.0;
        assert!(config.validate().is_ok());

        config.window.default_vertical_angle_degrees = -90.1;
        assert!(config.validate().is_err());

        config.window.default_vertical_angle_degrees = 90.1;
        assert!(config.validate().is_err());

        config.window.default_vertical_angle_degrees = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn window_texture_aa_modes_map_samples_and_format_labels() {
        for (mode, label) in WindowTextureAa::OPTIONS
            .into_iter()
            .zip(["1", "2x2", "4", "4x2", "8", "8x2", "16"])
        {
            assert_eq!(mode.to_string(), label);
            assert!(mode.samples_per_frame() <= mode.pattern_samples());
        }

        assert_eq!(WindowTextureAa::SuperSample2x2.samples_per_frame(), 2);
        assert!(WindowTextureAa::SuperSample2x2.is_temporal());
        assert_eq!(WindowTextureAa::SuperSample4.pattern_samples(), 4);
        assert!(!WindowTextureAa::SuperSample4.is_temporal());
        assert_eq!(WindowTextureAa::SuperSample4x2.samples_per_frame(), 4);
        assert!(WindowTextureAa::SuperSample4x2.is_temporal());
        assert_eq!(WindowTextureAa::SuperSample8.pattern_samples(), 8);
        assert!(!WindowTextureAa::SuperSample8.is_temporal());
        assert_eq!(WindowTextureAa::SuperSample8x2.samples_per_frame(), 8);
        assert!(WindowTextureAa::SuperSample8x2.is_temporal());
        assert_eq!(WindowTextureAa::SuperSample16.pattern_samples(), 16);
        assert!(!WindowTextureAa::SuperSample16.is_temporal());
    }

    #[test]
    fn validates_window_animation_and_collision_settings() {
        let mut config = AppConfig::default();
        config.window.animation_half_time_s = 1.0;
        config.window.collision_margin_m = 0.1;
        assert!(config.validate().is_ok());

        config.window.animation_half_time_s = 0.0;
        assert!(config.validate().is_ok());
        config.window.animation_half_time_s = 1.01;
        assert!(config.validate().is_err());
        config.window.animation_half_time_s = f32::NAN;
        assert!(config.validate().is_err());

        config.window.animation_half_time_s = 0.2;
        config.window.collision_margin_m = -0.01;
        assert!(config.validate().is_err());
        config.window.collision_margin_m = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_fullscreen_settings() {
        let mut config = AppConfig::default();
        config.window.fullscreen_max_width_degrees = 120.0;
        config.window.fullscreen_max_height_degrees = 80.0;
        config.window.fullscreen_environment_dim = 0.75;
        assert!(config.validate().is_ok());

        config.window.fullscreen_max_width_degrees = 0.0;
        assert!(config.validate().is_err());
        config.window.fullscreen_max_width_degrees = 171.0;
        assert!(config.validate().is_err());

        config.window.fullscreen_max_width_degrees = 100.0;
        config.window.fullscreen_max_height_degrees = f32::NAN;
        assert!(config.validate().is_err());
        config.window.fullscreen_max_height_degrees = 75.0;

        config.window.fullscreen_environment_dim = -0.01;
        assert!(config.validate().is_err());
        config.window.fullscreen_environment_dim = 1.01;
        assert!(config.validate().is_err());

        config.window.fullscreen_environment_dim = 0.5;
        config.window.maximized_max_width_degrees = 0.0;
        assert!(config.validate().is_err());
        config.window.maximized_max_width_degrees = 171.0;
        assert!(config.validate().is_err());

        config.window.maximized_max_width_degrees = 70.0;
        config.window.maximized_max_height_degrees = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_grabbed_border_style() {
        let mut config = AppConfig::default();
        config.window.grabbed_border_width_px = 8.0;
        config.window.grabbed_border_color = [0.2, 0.4, 0.6, 1.0];
        assert!(config.validate().is_ok());

        config.window.grabbed_border_width_px = 101.0;
        assert!(config.validate().is_err());

        config.window.grabbed_border_width_px = 4.0;
        config.window.grabbed_border_color[0] = f32::NAN;
        assert!(config.validate().is_err());
    }
}
