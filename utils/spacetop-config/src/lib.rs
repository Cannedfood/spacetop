use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use tempfile::NamedTempFile;

pub const DEFAULT_DISTANCE: f32 = 1.6;
pub const FALLBACK_FLOOR_HEIGHT: f32 = -1.3;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub background: BackgroundConfig,
    pub floor: FloorConfig,
    pub window: WindowConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct BackgroundConfig {
    pub image: String,
    pub brightness_stops: f32,
    pub rotation_degrees: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FloorConfig {
    pub height_m: f32,
    pub albedo: [f32; 4],
    pub roughness: f32,
    pub reflectance: f32,
    pub ray_count: u32,
    pub reflection_grain_size_m: f32,
    pub trace_through_transparent_windows: bool,
}

#[derive(Deserialize)]
#[serde(default)]
struct FloorConfigFields {
    height_m: f32,
    albedo: Option<Vec<f32>>,
    roughness: f32,
    reflectance: f32,
    transparency: Option<f32>,
    ray_count: u32,
    reflection_grain_size_m: f32,
    trace_through_transparent_windows: bool,
}

impl Default for FloorConfigFields {
    fn default() -> Self {
        let floor = FloorConfig::default();
        Self {
            height_m: floor.height_m,
            albedo: None,
            roughness: floor.roughness,
            reflectance: floor.reflectance,
            transparency: None,
            ray_count: floor.ray_count,
            reflection_grain_size_m: floor.reflection_grain_size_m,
            trace_through_transparent_windows: floor.trace_through_transparent_windows,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    pub default_distance_m: f32,
    pub pixels_per_degree: f32,
    pub padding_px: f32,
    pub animation_half_time_s: f32,
    pub collision_margin_m: f32,
    pub border_width_px: f32,
    pub border_color: [f32; 4],
    pub cursor_proximity_radius_px: f32,
    pub cursor_close_border_width_px: f32,
    pub cursor_close_border_color: [f32; 4],
    pub border_radius_px: f32,
    pub grabbed_border_width_px: f32,
    pub grabbed_border_color: [f32; 4],
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            image: "random".into(),
            brightness_stops: 0.0,
            rotation_degrees: 0.0,
        }
    }
}

impl Default for FloorConfig {
    fn default() -> Self {
        Self {
            height_m: FALLBACK_FLOOR_HEIGHT,
            albedo: [0.12, 0.12, 0.12, 0.75],
            roughness: 0.1,
            reflectance: 0.18,
            ray_count: 4,
            reflection_grain_size_m: 0.002,
            trace_through_transparent_windows: false,
        }
    }
}

impl<'de> Deserialize<'de> for FloorConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fields = FloorConfigFields::deserialize(deserializer)?;
        let defaults = Self::default();
        let legacy_alpha = || {
            fields
                .transparency
                .map_or(defaults.albedo[3], |transparency| 1.0 - transparency)
        };
        let albedo = match fields.albedo {
            Some(channels) => match channels.as_slice() {
                [red, green, blue] => [*red, *green, *blue, legacy_alpha()],
                [red, green, blue, alpha] => [*red, *green, *blue, *alpha],
                _ => {
                    return Err(D::Error::custom(
                        "floor.albedo must contain three legacy or four RGBA channels",
                    ));
                }
            },
            None => {
                let mut albedo = defaults.albedo;
                albedo[3] = legacy_alpha();
                albedo
            }
        };

        Ok(Self {
            height_m: fields.height_m,
            albedo,
            roughness: fields.roughness,
            reflectance: fields.reflectance,
            ray_count: fields.ray_count,
            reflection_grain_size_m: fields.reflection_grain_size_m,
            trace_through_transparent_windows: fields.trace_through_transparent_windows,
        })
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            default_distance_m: DEFAULT_DISTANCE,
            pixels_per_degree: 32.0,
            padding_px: 12.0,
            animation_half_time_s: 0.2,
            collision_margin_m: 0.04,
            border_width_px: 2.0,
            border_color: [0.58, 0.72, 0.74, 1.0],
            cursor_proximity_radius_px: 32.0,
            cursor_close_border_width_px: 4.0,
            cursor_close_border_color: [1.0, 0.9131, 0.0, 1.0],
            border_radius_px: 16.0,
            grabbed_border_width_px: 4.0,
            grabbed_border_color: [0.34, 0.82, 0.72, 1.0],
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
            self.window.default_distance_m.is_finite()
                && (0.1..=100.0).contains(&self.window.default_distance_m),
            "window.default_distance_m must be between 0.1 and 100 meters"
        );
        ensure!(
            self.window.pixels_per_degree.is_finite()
                && (1.0..=200.0).contains(&self.window.pixels_per_degree),
            "window.pixels_per_degree must be between 1 and 200"
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
        for (name, value, maximum) in [
            ("window.padding_px", self.window.padding_px, 500.0),
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
    fn older_window_settings_use_dodge_defaults() {
        let config: AppConfig =
            toml::from_str("[window]\ndefault_distance_m = 2.0\npixels_per_degree = 32.0\n")
                .unwrap();

        assert_eq!(config.window.animation_half_time_s, 0.2);
        assert_eq!(config.window.collision_margin_m, 0.04);
    }

    #[test]
    fn transparent_window_tracing_defaults_off_and_can_be_enabled() {
        let default = AppConfig::default();
        assert!(!default.floor.trace_through_transparent_windows);

        let enabled: AppConfig =
            toml::from_str("[floor]\ntrace_through_transparent_windows = true\n").unwrap();
        assert!(enabled.floor.trace_through_transparent_windows);

        let serialized = toml::to_string(&enabled).unwrap();
        let round_trip: AppConfig = toml::from_str(&serialized).unwrap();
        assert!(round_trip.floor.trace_through_transparent_windows);
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
