use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use iced::{
    Background, Border, Color, Element, Length, Theme,
    alignment::Horizontal,
    widget::{
        button, checkbox, column, container, image as iced_image, pick_list, row, scrollable,
        slider, text, text_input,
    },
};
use iced_aw::helpers::color_picker;
use spacetop_config::{AppConfig, ReflectionAtlasSize, WindowTextureAa};

mod view;

const RANDOM_BACKGROUND: &str = "Random";
const BACKGROUND_DOWNLOAD_URL: &str = "https://polyhaven.com/hdris";

const TEXT: Color = Color::from_rgb(0.92, 0.95, 0.96);
const MUTED: Color = Color::from_rgb(0.60, 0.69, 0.72);
const ACCENT: Color = Color::from_rgb(0.34, 0.82, 0.72);
const SURFACE: Color = Color::from_rgb(0.11, 0.15, 0.17);
const INPUT: Color = Color::from_rgb(0.07, 0.10, 0.12);
const BORDER: Color = Color::from_rgba(0.52, 0.73, 0.73, 0.22);

#[derive(Debug, Clone)]
enum Message {
    LauncherChanged(String),
    BackgroundChanged(String),
    OpenBackgroundFolder,
    RefreshBackgrounds,
    DownloadBackgrounds,
    OpenConfigFile,
    BackgroundBrightnessChanged(f32),
    BackgroundRotationChanged(f32),
    ChooseAlbedoColor,
    SubmitAlbedoColor(Color),
    CancelAlbedoColor,
    ChooseWindowBorderColor,
    SubmitWindowBorderColor(Color),
    CancelWindowBorderColor,
    ChooseCursorCloseBorderColor,
    SubmitCursorCloseBorderColor(Color),
    CancelCursorCloseBorderColor,
    ChooseGrabbedBorderColor,
    SubmitGrabbedBorderColor(Color),
    CancelGrabbedBorderColor,
    FloorHeightChanged(String),
    RoughnessChanged(f32),
    ReflectanceChanged(f32),
    ReflectionGrainSizeChanged(f32),
    RayCountChanged(u32),
    TransparentReflectionsChanged(bool),
    AmbientOcclusionChanged(bool),
    GroundRadiusChanged(f32),
    GroundFeatheringChanged(f32),
    WindowDistanceChanged(String),
    WindowVerticalAngleChanged(String),
    WindowPixelsPerDegreeChanged(String),
    WindowDisplayScaleChanged(f32),
    WindowTextureAaChanged(WindowTextureAa),
    ReflectionAtlasSizeChanged(ReflectionAtlasSize),
    WindowPaddingChanged(f32),
    WindowMarginChanged(f32),
    WindowAnimationHalfTimeChanged(f32),
    WindowCollisionMarginChanged(f32),
    WindowFullscreenMaxWidthChanged(f32),
    WindowFullscreenMaxHeightChanged(f32),
    WindowFullscreenEnvironmentDimChanged(f32),
    WindowMaximizedMaxWidthChanged(f32),
    WindowMaximizedMaxHeightChanged(f32),
    WindowBorderWidthChanged(f32),
    WindowBorderRadiusChanged(f32),
    WindowCursorProximityChanged(f32),
    WindowCursorCloseBorderWidthChanged(f32),
    WindowGrabbedBorderWidthChanged(f32),
    Reset(ResetTarget),
    Reload,
    Save,
}

#[derive(Debug, Clone, Copy)]
enum ResetTarget {
    ApplicationLauncher,
    BackgroundImage,
    BackgroundBrightness,
    BackgroundRotation,
    BackgroundSection,
    FloorHeight,
    FloorAlbedo,
    FloorRoughness,
    FloorReflectance,
    FloorRayCount,
    FloorTransparentReflections,
    FloorAmbientOcclusion,
    FloorReflectionGrainSize,
    FloorRadius,
    FloorFeathering,
    FloorSection,
    WindowDistance,
    WindowVerticalAngle,
    WindowPixelsPerDegree,
    WindowDisplayScale,
    WindowAnimationHalfTime,
    WindowCollisionMargin,
    WindowCursorProximity,
    WindowPadding,
    WindowMargin,
    WindowBorderRadius,
    WindowBorderWidth,
    WindowCursorCloseBorderWidth,
    WindowGrabbedBorderWidth,
    WindowBorderColor,
    WindowCursorCloseBorderColor,
    WindowGrabbedBorderColor,
    WindowPlacementSection,
    WindowFullscreenSection,
    WindowMaximizedSection,
    WindowResolutionSection,
    WindowBorderSection,
}

struct SettingsApp {
    config: AppConfig,
    backgrounds: Vec<PathBuf>,
    background_preview_source: Option<image::Rgb32FImage>,
    background_preview: Option<iced_image::Handle>,
    background_preview_message: String,
    show_albedo_picker: bool,
    show_window_border_picker: bool,
    show_cursor_close_border_picker: bool,
    show_grabbed_border_picker: bool,
    floor_height: String,
    window_distance: String,
    window_vertical_angle: String,
    window_pixels_per_degree: String,
    window_display_scale: f32,
    status: String,
    status_is_error: bool,
}

impl SettingsApp {
    fn new() -> Self {
        match AppConfig::load() {
            Ok(config) => Self::from_config(config, "Configuration loaded".into(), false),
            Err(error) => Self::from_config(
                AppConfig::default(),
                format!("Could not load config: {error:#}"),
                true,
            ),
        }
    }

    fn from_config(config: AppConfig, status: String, status_is_error: bool) -> Self {
        let backgrounds = background_files();
        let floor_height = format!("{}", config.floor.height_m);
        let window_distance = format!("{}", config.window.default_distance_m);
        let window_vertical_angle = format!("{}", config.window.default_vertical_angle_degrees);
        let window_pixels_per_degree = format!("{}", config.window.pixels_per_degree);
        let window_display_scale = config.window.display_scale;
        let mut app = Self {
            config,
            backgrounds,
            background_preview_source: None,
            background_preview: None,
            background_preview_message: String::new(),
            show_albedo_picker: false,
            show_window_border_picker: false,
            show_cursor_close_border_picker: false,
            show_grabbed_border_picker: false,
            floor_height,
            window_distance,
            window_vertical_angle,
            window_pixels_per_degree,
            window_display_scale,
            status,
            status_is_error,
        };
        app.load_background_preview();
        app
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        let editing = matches!(
            &message,
            Message::LauncherChanged(_)
                | Message::BackgroundChanged(_)
                | Message::BackgroundBrightnessChanged(_)
                | Message::BackgroundRotationChanged(_)
                | Message::FloorHeightChanged(_)
                | Message::SubmitAlbedoColor(_)
                | Message::SubmitWindowBorderColor(_)
                | Message::SubmitCursorCloseBorderColor(_)
                | Message::SubmitGrabbedBorderColor(_)
                | Message::RoughnessChanged(_)
                | Message::ReflectanceChanged(_)
                | Message::ReflectionGrainSizeChanged(_)
                | Message::RayCountChanged(_)
                | Message::TransparentReflectionsChanged(_)
                | Message::AmbientOcclusionChanged(_)
                | Message::WindowDistanceChanged(_)
                | Message::WindowVerticalAngleChanged(_)
                | Message::WindowPixelsPerDegreeChanged(_)
                | Message::WindowDisplayScaleChanged(_)
                | Message::WindowTextureAaChanged(_)
                | Message::ReflectionAtlasSizeChanged(_)
                | Message::WindowPaddingChanged(_)
                | Message::WindowMarginChanged(_)
                | Message::WindowAnimationHalfTimeChanged(_)
                | Message::WindowCollisionMarginChanged(_)
                | Message::WindowFullscreenMaxWidthChanged(_)
                | Message::WindowFullscreenMaxHeightChanged(_)
                | Message::WindowFullscreenEnvironmentDimChanged(_)
                | Message::WindowMaximizedMaxWidthChanged(_)
                | Message::WindowMaximizedMaxHeightChanged(_)
                | Message::WindowBorderWidthChanged(_)
                | Message::WindowBorderRadiusChanged(_)
                | Message::WindowCursorProximityChanged(_)
                | Message::WindowCursorCloseBorderWidthChanged(_)
                | Message::WindowGrabbedBorderWidthChanged(_)
                | Message::Reset(_)
        );
        match message {
            Message::LauncherChanged(value) => self.config.application.launcher = value,
            Message::BackgroundChanged(selection) => {
                if selection == RANDOM_BACKGROUND {
                    self.config.background.image = "random".into();
                } else if let Some(path) = self.backgrounds.iter().find(|path| {
                    path.file_name()
                        .is_some_and(|name| name == selection.as_str())
                }) {
                    self.config.background.image = format!(
                        "~/.config/spacetop/backgrounds/{}",
                        path.file_name().unwrap().to_string_lossy()
                    );
                }
                self.load_background_preview();
            }
            Message::OpenBackgroundFolder => self.open_background_folder(),
            Message::RefreshBackgrounds => {
                self.backgrounds = background_files();
                self.status = if self.backgrounds.is_empty() {
                    "No EXR backgrounds found".into()
                } else {
                    format!("Found {} EXR background(s)", self.backgrounds.len())
                };
                self.status_is_error = false;
            }
            Message::DownloadBackgrounds => match open_with_xdg(BACKGROUND_DOWNLOAD_URL) {
                Ok(()) => {
                    self.status = "Opened Poly Haven in your browser".into();
                    self.status_is_error = false;
                }
                Err(error) => {
                    self.status = format!("Could not open Poly Haven: {error}");
                    self.status_is_error = true;
                }
            },
            Message::OpenConfigFile => self.open_config_file(),
            Message::BackgroundBrightnessChanged(value) => {
                self.config.background.brightness_stops = value;
                self.update_background_preview();
            }
            Message::BackgroundRotationChanged(value) => {
                self.config.background.rotation_degrees = value;
                self.update_background_preview();
            }
            Message::ChooseAlbedoColor => self.show_albedo_picker = true,
            Message::SubmitAlbedoColor(color) => {
                self.config.floor.albedo = [color.r, color.g, color.b, color.a];
                self.show_albedo_picker = false;
            }
            Message::CancelAlbedoColor => self.show_albedo_picker = false,
            Message::ChooseWindowBorderColor => self.show_window_border_picker = true,
            Message::SubmitWindowBorderColor(color) => {
                self.config.window.border_color = [color.r, color.g, color.b, color.a];
                self.show_window_border_picker = false;
            }
            Message::CancelWindowBorderColor => self.show_window_border_picker = false,
            Message::ChooseCursorCloseBorderColor => self.show_cursor_close_border_picker = true,
            Message::SubmitCursorCloseBorderColor(color) => {
                self.config.window.cursor_close_border_color = [color.r, color.g, color.b, color.a];
                self.show_cursor_close_border_picker = false;
            }
            Message::CancelCursorCloseBorderColor => self.show_cursor_close_border_picker = false,
            Message::ChooseGrabbedBorderColor => self.show_grabbed_border_picker = true,
            Message::SubmitGrabbedBorderColor(color) => {
                self.config.window.grabbed_border_color = [color.r, color.g, color.b, color.a];
                self.show_grabbed_border_picker = false;
            }
            Message::CancelGrabbedBorderColor => self.show_grabbed_border_picker = false,
            Message::FloorHeightChanged(value) => self.floor_height = value,
            Message::RoughnessChanged(value) => self.config.floor.roughness = value,
            Message::ReflectanceChanged(value) => self.config.floor.reflectance = value,
            Message::ReflectionGrainSizeChanged(value_mm) => {
                self.config.floor.reflection_grain_size_m = value_mm / 1000.0;
            }
            Message::RayCountChanged(value) => self.config.floor.ray_count = value,
            Message::TransparentReflectionsChanged(value) => {
                self.config.floor.trace_through_transparent_windows = value;
            }
            Message::AmbientOcclusionChanged(value) => {
                self.config.floor.ambient_occlusion = value;
            }
            Message::GroundRadiusChanged(value) => self.config.floor.radius_degrees = value,
            Message::GroundFeatheringChanged(value) => self.config.floor.feathering_m = value,
            Message::WindowDistanceChanged(value) => self.window_distance = value,
            Message::WindowVerticalAngleChanged(value) => self.window_vertical_angle = value,
            Message::WindowPixelsPerDegreeChanged(value) => self.window_pixels_per_degree = value,
            Message::WindowDisplayScaleChanged(value) => self.window_display_scale = value,
            Message::WindowTextureAaChanged(value) => self.config.window.texture_aa = value,
            Message::ReflectionAtlasSizeChanged(value) => {
                self.config.window.reflection_atlas_size = value
            }
            Message::WindowPaddingChanged(value) => self.config.window.padding_px = value,
            Message::WindowMarginChanged(value) => self.config.window.margin_px = value,
            Message::WindowAnimationHalfTimeChanged(value) => {
                self.config.window.animation_half_time_s = value
            }
            Message::WindowCollisionMarginChanged(value) => {
                self.config.window.collision_margin_m = value / 100.0
            }
            Message::WindowFullscreenMaxWidthChanged(value) => {
                self.config.window.fullscreen_max_width_degrees = value
            }
            Message::WindowFullscreenMaxHeightChanged(value) => {
                self.config.window.fullscreen_max_height_degrees = value
            }
            Message::WindowFullscreenEnvironmentDimChanged(value) => {
                self.config.window.fullscreen_environment_dim = value / 100.0
            }
            Message::WindowMaximizedMaxWidthChanged(value) => {
                self.config.window.maximized_max_width_degrees = value
            }
            Message::WindowMaximizedMaxHeightChanged(value) => {
                self.config.window.maximized_max_height_degrees = value
            }
            Message::WindowBorderWidthChanged(value) => self.config.window.border_width_px = value,
            Message::WindowBorderRadiusChanged(value) => {
                self.config.window.border_radius_px = value
            }
            Message::WindowCursorProximityChanged(value) => {
                self.config.window.cursor_proximity_radius_px = value
            }
            Message::WindowCursorCloseBorderWidthChanged(value) => {
                self.config.window.cursor_close_border_width_px = value
            }
            Message::WindowGrabbedBorderWidthChanged(value) => {
                self.config.window.grabbed_border_width_px = value
            }
            Message::Reset(target) => self.reset(target),
            Message::Reload => match AppConfig::load() {
                Ok(config) => {
                    *self = Self::from_config(config, "Configuration reloaded".into(), false)
                }
                Err(error) => {
                    self.status = format!("Could not reload config: {error:#}");
                    self.status_is_error = true;
                }
            },
            Message::Save => self.save(),
        }
        if editing {
            self.status = "Unsaved changes".into();
            self.status_is_error = false;
        }
        iced::Task::none()
    }

    fn atlas_size_picker(&self) -> Element<'_, Message> {
        row![
            text("ATLAS SIZE").size(11).color(MUTED),
            pick_list(
                ReflectionAtlasSize::OPTIONS,
                Some(self.config.window.reflection_atlas_size),
                Message::ReflectionAtlasSizeChanged,
            )
            .width(Length::Fill),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center)
        .into()
    }

    fn reset(&mut self, target: ResetTarget) {
        let defaults = AppConfig::default();
        match target {
            ResetTarget::ApplicationLauncher => {
                self.config.application.launcher = defaults.application.launcher;
            }
            ResetTarget::BackgroundImage => {
                self.config.background.image = defaults.background.image;
                self.load_background_preview();
            }
            ResetTarget::BackgroundBrightness => {
                self.config.background.brightness_stops = defaults.background.brightness_stops;
                self.update_background_preview();
            }
            ResetTarget::BackgroundRotation => {
                self.config.background.rotation_degrees = defaults.background.rotation_degrees;
                self.update_background_preview();
            }
            ResetTarget::BackgroundSection => {
                self.config.background = defaults.background;
                self.load_background_preview();
            }
            ResetTarget::FloorHeight => {
                self.config.floor.height_m = defaults.floor.height_m;
                self.floor_height = defaults.floor.height_m.to_string();
            }
            ResetTarget::FloorAlbedo => {
                self.config.floor.albedo = defaults.floor.albedo;
                self.show_albedo_picker = false;
            }
            ResetTarget::FloorRoughness => {
                self.config.floor.roughness = defaults.floor.roughness;
            }
            ResetTarget::FloorReflectance => {
                self.config.floor.reflectance = defaults.floor.reflectance;
            }
            ResetTarget::FloorRayCount => self.config.floor.ray_count = defaults.floor.ray_count,
            ResetTarget::FloorTransparentReflections => {
                self.config.floor.trace_through_transparent_windows =
                    defaults.floor.trace_through_transparent_windows;
            }
            ResetTarget::FloorAmbientOcclusion => {
                self.config.floor.ambient_occlusion = defaults.floor.ambient_occlusion;
            }
            ResetTarget::FloorReflectionGrainSize => {
                self.config.floor.reflection_grain_size_m = defaults.floor.reflection_grain_size_m;
            }
            ResetTarget::FloorRadius => {
                self.config.floor.radius_degrees = defaults.floor.radius_degrees;
            }
            ResetTarget::FloorFeathering => {
                self.config.floor.feathering_m = defaults.floor.feathering_m;
            }
            ResetTarget::FloorSection => {
                self.floor_height = defaults.floor.height_m.to_string();
                self.config.floor = defaults.floor;
                self.show_albedo_picker = false;
            }
            ResetTarget::WindowDistance => {
                self.config.window.default_distance_m = defaults.window.default_distance_m;
                self.window_distance = defaults.window.default_distance_m.to_string();
            }
            ResetTarget::WindowVerticalAngle => {
                self.config.window.default_vertical_angle_degrees =
                    defaults.window.default_vertical_angle_degrees;
                self.window_vertical_angle =
                    defaults.window.default_vertical_angle_degrees.to_string();
            }
            ResetTarget::WindowPixelsPerDegree => {
                self.config.window.pixels_per_degree = defaults.window.pixels_per_degree;
                self.window_pixels_per_degree = defaults.window.pixels_per_degree.to_string();
            }
            ResetTarget::WindowDisplayScale => {
                self.config.window.display_scale = defaults.window.display_scale;
                self.window_display_scale = defaults.window.display_scale;
            }
            ResetTarget::WindowAnimationHalfTime => {
                self.config.window.animation_half_time_s = defaults.window.animation_half_time_s;
            }
            ResetTarget::WindowCollisionMargin => {
                self.config.window.collision_margin_m = defaults.window.collision_margin_m;
            }
            ResetTarget::WindowCursorProximity => {
                self.config.window.cursor_proximity_radius_px =
                    defaults.window.cursor_proximity_radius_px;
            }
            ResetTarget::WindowPadding => {
                self.config.window.padding_px = defaults.window.padding_px
            }
            ResetTarget::WindowMargin => self.config.window.margin_px = defaults.window.margin_px,
            ResetTarget::WindowBorderRadius => {
                self.config.window.border_radius_px = defaults.window.border_radius_px;
            }
            ResetTarget::WindowBorderWidth => {
                self.config.window.border_width_px = defaults.window.border_width_px;
            }
            ResetTarget::WindowCursorCloseBorderWidth => {
                self.config.window.cursor_close_border_width_px =
                    defaults.window.cursor_close_border_width_px;
            }
            ResetTarget::WindowGrabbedBorderWidth => {
                self.config.window.grabbed_border_width_px =
                    defaults.window.grabbed_border_width_px;
            }
            ResetTarget::WindowBorderColor => {
                self.config.window.border_color = defaults.window.border_color;
                self.show_window_border_picker = false;
            }
            ResetTarget::WindowCursorCloseBorderColor => {
                self.config.window.cursor_close_border_color =
                    defaults.window.cursor_close_border_color;
                self.show_cursor_close_border_picker = false;
            }
            ResetTarget::WindowGrabbedBorderColor => {
                self.config.window.grabbed_border_color = defaults.window.grabbed_border_color;
                self.show_grabbed_border_picker = false;
            }
            ResetTarget::WindowPlacementSection => {
                self.config.window.default_distance_m = defaults.window.default_distance_m;
                self.config.window.default_vertical_angle_degrees =
                    defaults.window.default_vertical_angle_degrees;
                self.config.window.animation_half_time_s = defaults.window.animation_half_time_s;
                self.config.window.collision_margin_m = defaults.window.collision_margin_m;
                self.window_distance = defaults.window.default_distance_m.to_string();
                self.window_vertical_angle =
                    defaults.window.default_vertical_angle_degrees.to_string();
            }
            ResetTarget::WindowFullscreenSection => {
                self.config.window.fullscreen_max_width_degrees =
                    defaults.window.fullscreen_max_width_degrees;
                self.config.window.fullscreen_max_height_degrees =
                    defaults.window.fullscreen_max_height_degrees;
                self.config.window.fullscreen_environment_dim =
                    defaults.window.fullscreen_environment_dim;
            }
            ResetTarget::WindowMaximizedSection => {
                self.config.window.maximized_max_width_degrees =
                    defaults.window.maximized_max_width_degrees;
                self.config.window.maximized_max_height_degrees =
                    defaults.window.maximized_max_height_degrees;
            }
            ResetTarget::WindowResolutionSection => {
                self.config.window.pixels_per_degree = defaults.window.pixels_per_degree;
                self.config.window.texture_aa = defaults.window.texture_aa;
                self.config.window.reflection_atlas_size = defaults.window.reflection_atlas_size;
                self.window_pixels_per_degree = defaults.window.pixels_per_degree.to_string();
                self.config.window.display_scale = defaults.window.display_scale;
                self.window_display_scale = defaults.window.display_scale;
            }
            ResetTarget::WindowBorderSection => {
                self.config.window.padding_px = defaults.window.padding_px;
                self.config.window.margin_px = defaults.window.margin_px;
                self.config.window.border_radius_px = defaults.window.border_radius_px;
                self.config.window.border_width_px = defaults.window.border_width_px;
                self.config.window.cursor_proximity_radius_px =
                    defaults.window.cursor_proximity_radius_px;
                self.config.window.cursor_close_border_width_px =
                    defaults.window.cursor_close_border_width_px;
                self.config.window.border_color = defaults.window.border_color;
                self.config.window.cursor_close_border_color =
                    defaults.window.cursor_close_border_color;
                self.config.window.grabbed_border_width_px =
                    defaults.window.grabbed_border_width_px;
                self.config.window.grabbed_border_color = defaults.window.grabbed_border_color;
                self.show_window_border_picker = false;
                self.show_cursor_close_border_picker = false;
                self.show_grabbed_border_picker = false;
            }
        }
    }

    fn open_background_folder(&mut self) {
        let Some(directory) = background_directory() else {
            self.status = "Could not locate the backgrounds folder: HOME is not set".into();
            self.status_is_error = true;
            return;
        };
        if let Err(error) = fs::create_dir_all(&directory) {
            self.status = format!("Could not create backgrounds folder: {error}");
            self.status_is_error = true;
            return;
        }

        self.backgrounds = background_files();
        match open_with_xdg(&directory) {
            Ok(()) => {
                self.status = "Opened backgrounds folder".into();
                self.status_is_error = false;
            }
            Err(error) => {
                self.status = format!("Could not open backgrounds folder: {error}");
                self.status_is_error = true;
            }
        }
    }

    fn open_config_file(&mut self) {
        match AppConfig::path() {
            Ok(path) => match open_with_xdg(&path) {
                Ok(()) => {
                    self.status = "Opened configuration file".into();
                    self.status_is_error = false;
                }
                Err(error) => {
                    self.status = format!("Could not open configuration file: {error}");
                    self.status_is_error = true;
                }
            },
            Err(error) => {
                self.status = format!("Could not locate configuration file: {error:#}");
                self.status_is_error = true;
            }
        }
    }

    fn load_background_preview(&mut self) {
        self.background_preview_source = None;
        self.background_preview = None;
        if self.config.background.image == "random" {
            self.background_preview_message =
                "Random chooses an image when Spacetop starts. Select an EXR to preview.".into();
            return;
        }

        let path = expand_background_path(&self.config.background.image);
        match image::open(&path) {
            Ok(decoded) => {
                self.background_preview_source = Some(decoded.thumbnail(640, 320).to_rgb32f());
                self.background_preview_message.clear();
                self.update_background_preview();
            }
            Err(error) => {
                self.background_preview_message = format!("Could not load preview: {error}");
            }
        }
    }

    fn update_background_preview(&mut self) {
        self.background_preview = self.background_preview_source.as_ref().map(|source| {
            let pixels = tone_mapped_preview_pixels(
                source,
                self.config.background.brightness_stops,
                self.config.background.rotation_degrees,
            );
            iced_image::Handle::from_rgba(source.width(), source.height(), pixels)
        });
    }

    fn save(&mut self) {
        let config = match self.config_from_fields() {
            Ok(config) => config,
            Err(error) => {
                self.status = error;
                self.status_is_error = true;
                return;
            }
        };
        let result = AppConfig::path().and_then(|path| config.save_to(&path));
        match result {
            Ok(()) => {
                self.config = config;
                self.status = "Saved. Spacetop will reload shortly.".into();
                self.status_is_error = false;
            }
            Err(error) => {
                self.status = format!("Could not save config: {error:#}");
                self.status_is_error = true;
            }
        }
    }

    fn config_from_fields(&self) -> Result<AppConfig, String> {
        let mut config = self.config.clone();
        config.floor.height_m = parse_number("Fallback floor height", &self.floor_height)?;
        config.window.default_distance_m =
            parse_number("Default window distance", &self.window_distance)?;
        config.window.default_vertical_angle_degrees =
            parse_number("Default window vertical angle", &self.window_vertical_angle)?;
        config.window.pixels_per_degree =
            parse_number("Window pixels per degree", &self.window_pixels_per_degree)?;
        config.window.display_scale = self.window_display_scale;
        config
            .validate()
            .map_err(|error| format!("Invalid settings: {error}"))?;
        Ok(config)
    }
}

fn background_directory() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/spacetop/backgrounds"))
}

fn background_files() -> Vec<PathBuf> {
    let Some(directory) = background_directory() else {
        return Vec::new();
    };
    background_files_in(&directory)
}

fn background_files_in(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_file = entry.file_type().ok()?.is_file();
            let is_exr = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exr"));
            (is_file && is_exr && path.file_name().and_then(OsStr::to_str).is_some())
                .then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    });
    files
}

fn expand_background_path(path: &str) -> PathBuf {
    if let Some(relative) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(relative);
    }
    PathBuf::from(path)
}

fn tone_mapped_preview_pixels(
    source: &image::Rgb32FImage,
    brightness_stops: f32,
    rotation_degrees: f32,
) -> Vec<u8> {
    let exposure = brightness_stops.exp2();
    let width = source.width();
    let height = source.height();
    let shift = (rotation_degrees.rem_euclid(360.0) / 360.0 * width as f32).round() as u32;
    let mut output = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let pixel = source.get_pixel((x + shift) % width, y);
            let encode = |channel: f32| {
                let exposed = (channel * exposure).max(0.0);
                let mapped = if exposed.is_infinite() {
                    1.0
                } else {
                    exposed / (1.0 + exposed)
                };
                let srgb = if mapped <= 0.003_130_8 {
                    mapped * 12.92
                } else {
                    1.055 * mapped.powf(1.0 / 2.4) - 0.055
                };
                (srgb * 255.0).round().clamp(0.0, 255.0) as u8
            };
            output.extend([encode(pixel[0]), encode(pixel[1]), encode(pixel[2]), 255]);
        }
    }
    output
}

fn open_with_xdg(target: impl AsRef<OsStr>) -> Result<(), std::io::Error> {
    Command::new("xdg-open").arg(target).spawn().map(|_| ())
}

fn parse_number(label: &str, value: &str) -> Result<f32, String> {
    value
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("{label} must be a number"))
}

fn number_is_default(value: &str, default: f32) -> bool {
    value.parse::<f32>().is_ok_and(|value| value == default)
}

fn section<'a>(
    title: &'a str,
    content: impl Into<Element<'a, Message>>,
    is_default: bool,
    reset_target: ResetTarget,
) -> Element<'a, Message> {
    container(
        column![
            row![
                text(title).size(11).color(ACCENT).width(Length::Fill),
                reset_button(is_default, reset_target),
            ]
            .align_y(iced::Alignment::Center),
            content.into(),
        ]
        .spacing(16),
    )
    .width(Length::Fill)
    .padding(18)
    .style(|_| container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn labeled_input<'a>(
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    on_input: impl Fn(String) -> Message + 'static,
    is_default: bool,
    reset_target: ResetTarget,
) -> Element<'a, Message> {
    column![
        text(label).size(11).color(MUTED),
        row![
            styled_input(placeholder, value, on_input),
            reset_button(is_default, reset_target),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}

fn styled_input<'a>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'static,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input(on_input)
        .padding([9, 11])
        .size(14)
        .style(|theme, status| {
            let mut style = text_input::default(theme, status);
            style.background = Background::Color(INPUT);
            style.border = Border {
                color: BORDER,
                width: 1.0,
                radius: 6.0.into(),
            };
            style
        })
        .into()
}

fn color_swatch(color: Color) -> Element<'static, Message> {
    row![
        container(text(""))
            .width(Length::Fixed(42.0))
            .height(Length::Fixed(32.0))
            .style(move |_| container::Style {
                background: Some(Background::Color(color)),
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            }),
        text("Pick color").size(13).color(TEXT),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center)
    .into()
}

#[allow(clippy::too_many_arguments)]
fn slider_row<'a>(
    label: &'a str,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    on_change: impl Fn(f32) -> Message + 'static,
    is_default: bool,
    reset_target: ResetTarget,
) -> Element<'a, Message> {
    row![
        text(label)
            .size(11)
            .color(MUTED)
            .width(Length::Fixed(126.0)),
        slider(min..=max, value, on_change)
            .step(step)
            .width(Length::Fill),
        text(format!("{value:.2}"))
            .size(13)
            .color(TEXT)
            .width(Length::Fixed(34.0))
            .align_x(Horizontal::Right),
        reset_button(is_default, reset_target),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center)
    .into()
}

fn quiet_button(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(SURFACE)),
        text_color: TEXT,
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn reset_button(is_default: bool, target: ResetTarget) -> Element<'static, Message> {
    button(text("Reset").size(11))
        .on_press_maybe((!is_default).then_some(Message::Reset(target)))
        .padding([6, 8])
        .style(reset_button_style)
        .into()
}

fn reset_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let disabled = matches!(status, button::Status::Disabled);
    button::Style {
        background: Some(Background::Color(if disabled {
            Color::from_rgb(0.13, 0.16, 0.17)
        } else {
            SURFACE
        })),
        text_color: if disabled { MUTED } else { TEXT },
        border: Border {
            color: if disabled {
                Color::from_rgba(0.52, 0.73, 0.73, 0.10)
            } else {
                BORDER
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn accent_button(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(ACCENT)),
        text_color: Color::from_rgb(0.035, 0.10, 0.095),
        border: Border {
            color: ACCENT,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn main() -> iced::Result {
    iced::application(SettingsApp::new, SettingsApp::update, SettingsApp::view)
        .font(iced_aw::ICED_AW_FONT_BYTES)
        .title("Space Settings")
        .window_size((760.0, 820.0))
        .centered()
        .theme(Theme::Dark)
        .run()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
