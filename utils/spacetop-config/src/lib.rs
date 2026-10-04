use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AppConfig {
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
    pub reflection_grain_size_m: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    pub pixels_per_degree: f32,
    pub texture_aa: WindowTextureAa,

    pub default_distance_m: f32,
    pub default_vertical_angle_degrees: f32,
    pub animation_half_time_s: f32,
    pub collision_margin_m: f32,

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
        self.effective_padding_px() + self.effective_margin_px()
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
            reflection_grain_size_m: 0.001,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            pixels_per_degree: 30.0,
            texture_aa: WindowTextureAa::default(),

            default_distance_m: 1.6,
            default_vertical_angle_degrees: -10.0,
            animation_half_time_s: 0.2,
            collision_margin_m: 0.04,

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
    fn project_defaults_match_current_settings() {
        let config = AppConfig::default();

        assert_eq!(config.application.launcher, "spacelauncher");
        assert_eq!(config.background.image, "random");
        assert_eq!(config.background.brightness_stops, -0.1);
        assert_eq!(config.background.rotation_degrees, 0.0);
        assert_eq!(config.floor.height_m, -1.3);
        assert_eq!(config.floor.albedo, [0.5367573, 0.5114081, 0.5114081, 1.0]);
        assert_eq!(config.floor.roughness, 0.06);
        assert_eq!(config.floor.reflectance, 0.15);
        assert_eq!(config.floor.ray_count, 1);
        assert_eq!(config.floor.reflection_grain_size_m, 0.001);
        assert!(config.floor.trace_through_transparent_windows);
        assert_eq!(config.window.default_distance_m, 1.6);
        assert_eq!(config.window.default_vertical_angle_degrees, 0.0);
        assert_eq!(config.window.pixels_per_degree, 32.0);
        assert_eq!(config.window.texture_aa, WindowTextureAa::SuperSample2x2);
        assert_eq!(config.window.padding_px, 0.0);
        assert_eq!(config.window.margin_px, 4.0);
        assert_eq!(config.window.animation_half_time_s, 0.2);
        assert_eq!(config.window.collision_margin_m, 0.04);
        assert_eq!(config.window.border_width_px, 0.0);
        assert_eq!(
            config.window.border_color,
            [0.9911504, 0.9911504, 0.9911504, 0.0]
        );
        assert_eq!(config.window.cursor_proximity_radius_px, 301.0);
        assert_eq!(config.window.cursor_close_border_width_px, 2.0);
        assert_eq!(
            config.window.cursor_close_border_color,
            [0.2485206, 0.9534924, 0.8007485, 0.5680294]
        );
        assert_eq!(config.window.border_radius_px, 42.0);
        assert_eq!(config.window.grabbed_border_width_px, 1.0);
        assert_eq!(config.window.grabbed_border_color, [0.34, 0.82, 0.72, 1.0]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_an_empty_launcher_executable() {
        let mut config = AppConfig::default();
        config.application.launcher = "  ".into();

        assert!(config.validate().is_err());
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
    fn transparent_window_tracing_defaults_on_and_can_be_disabled() {
        let default = AppConfig::default();
        assert!(default.floor.trace_through_transparent_windows);

        let disabled: AppConfig =
            toml::from_str("[floor]\ntrace_through_transparent_windows = false\n").unwrap();
        assert!(!disabled.floor.trace_through_transparent_windows);

        let serialized = toml::to_string(&disabled).unwrap();
        let round_trip: AppConfig = toml::from_str(&serialized).unwrap();
        assert!(!round_trip.floor.trace_through_transparent_windows);
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
    fn window_texture_aa_modes_round_trip_and_map_sample_counts() {
        for (mode, label) in WindowTextureAa::OPTIONS
            .into_iter()
            .zip(["1", "2x2", "4", "4x2", "8", "8x2", "16"])
        {
            let mut config = AppConfig::default();
            config.window.texture_aa = mode;
            let serialized = toml::to_string(&config).unwrap();
            let round_trip: AppConfig = toml::from_str(&serialized).unwrap();
            assert_eq!(round_trip.window.texture_aa, mode);
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

    #[test]
    fn clamps_padding_and_margin_to_half_the_widest_border() {
        let mut config = AppConfig::default();
        config.window.border_width_px = 6.0;
        config.window.cursor_close_border_width_px = 10.0;
        config.window.grabbed_border_width_px = 8.0;
        config.window.padding_px = 1.0;
        config.window.margin_px = 2.0;

        assert_eq!(config.window.effective_padding_px(), 5.0);
        assert_eq!(config.window.effective_margin_px(), 5.0);
        assert_eq!(config.window.grab_reach_px(), 10.0);

        config.window.padding_px = 7.0;
        config.window.margin_px = 9.0;
        assert_eq!(config.window.effective_padding_px(), 7.0);
        assert_eq!(config.window.effective_margin_px(), 9.0);
        assert_eq!(config.window.grab_reach_px(), 16.0);
    }
}
