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
use spacetop_config::{AppConfig, ReflectionAtlasSize, ReflectionTextures, WindowTextureAa};

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
    WindowTextureAaChanged(WindowTextureAa),
    ReflectionTexturesChanged(ReflectionTextures),
    ReflectionAtlasSizeChanged(ReflectionAtlasSize),
    WindowPaddingChanged(f32),
    WindowMarginChanged(f32),
    WindowAnimationHalfTimeChanged(f32),
    WindowCollisionMarginChanged(f32),
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
                | Message::WindowTextureAaChanged(_)
                | Message::ReflectionTexturesChanged(_)
                | Message::ReflectionAtlasSizeChanged(_)
                | Message::WindowPaddingChanged(_)
                | Message::WindowMarginChanged(_)
                | Message::WindowAnimationHalfTimeChanged(_)
                | Message::WindowCollisionMarginChanged(_)
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
            Message::WindowTextureAaChanged(value) => self.config.window.texture_aa = value,
            Message::ReflectionTexturesChanged(value) => {
                self.config.window.reflection_textures = value
            }
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

    fn atlas_size_picker(&self) -> Option<Element<'_, Message>> {
        (self.config.window.reflection_textures == ReflectionTextures::Atlas).then(|| {
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
        })
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
            ResetTarget::WindowResolutionSection => {
                self.config.window.pixels_per_degree = defaults.window.pixels_per_degree;
                self.config.window.texture_aa = defaults.window.texture_aa;
                self.config.window.reflection_textures = defaults.window.reflection_textures;
                self.config.window.reflection_atlas_size = defaults.window.reflection_atlas_size;
                self.window_pixels_per_degree = defaults.window.pixels_per_degree.to_string();
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
        config
            .validate()
            .map_err(|error| format!("Invalid settings: {error}"))?;
        Ok(config)
    }

    fn view(&self) -> Element<'_, Message> {
        let defaults = AppConfig::default();
        let mut background_options = vec![RANDOM_BACKGROUND.to_owned()];
        background_options.extend(
            self.backgrounds
                .iter()
                .filter_map(|path| path.file_name().and_then(OsStr::to_str).map(str::to_owned)),
        );
        let selected_background = if self.config.background.image == "random" {
            Some(RANDOM_BACKGROUND.to_owned())
        } else {
            let configured_path = expand_background_path(&self.config.background.image);
            self.backgrounds
                .iter()
                .find(|path| **path == configured_path)
                .and_then(|path| path.file_name())
                .and_then(OsStr::to_str)
                .map(str::to_owned)
        };
        let background_preview: Element<'_, Message> =
            if let Some(handle) = &self.background_preview {
                column![
                    container(
                        iced_image::Image::new(handle.clone())
                            .width(Length::Fill)
                            .height(Length::Fixed(164.0))
                            .content_fit(iced::ContentFit::Contain)
                    )
                    .width(Length::Fill)
                    .height(Length::Fixed(164.0))
                    .padding(5)
                    .style(|_| container::Style {
                        background: Some(Background::Color(INPUT)),
                        border: Border {
                            color: BORDER,
                            width: 1.0,
                            radius: 6.0.into(),
                        },
                        ..Default::default()
                    }),
                    text(format!(
                        "Exposure preview: {:+.1} EV",
                        self.config.background.brightness_stops
                    ))
                    .size(11)
                    .color(MUTED),
                ]
                .spacing(6)
                .into()
            } else {
                container(
                    text(self.background_preview_message.as_str())
                        .size(12)
                        .color(MUTED),
                )
                .width(Length::Fill)
                .height(Length::Fixed(164.0))
                .padding(12)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(INPUT)),
                    border: Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                })
                .into()
            };
        let environment = section(
            "BACKGROUND",
            column![
                row![
                    column![
                        text("IMAGE SOURCE").size(11).color(MUTED),
                        pick_list(
                            background_options,
                            selected_background,
                            Message::BackgroundChanged,
                        )
                        .placeholder("Select an EXR background")
                        .padding([9, 11])
                        .width(Length::Fill),
                    ]
                    .spacing(8)
                    .width(Length::Fill),
                    reset_button(
                        self.config.background.image == defaults.background.image,
                        ResetTarget::BackgroundImage,
                    ),
                ]
                .spacing(8)
                .align_y(iced::Alignment::End),
                background_preview,
                slider_row(
                    "SKYBOX BRIGHTNESS (EV)",
                    self.config.background.brightness_stops,
                    -8.0,
                    8.0,
                    0.1,
                    Message::BackgroundBrightnessChanged,
                    self.config.background.brightness_stops == defaults.background.brightness_stops,
                    ResetTarget::BackgroundBrightness,
                ),
                slider_row(
                    "SKYBOX ROTATION (DEG)",
                    self.config.background.rotation_degrees,
                    0.0,
                    360.0,
                    1.0,
                    Message::BackgroundRotationChanged,
                    self.config.background.rotation_degrees == defaults.background.rotation_degrees,
                    ResetTarget::BackgroundRotation,
                ),
                row![
                    button(text("Open Folder").size(13))
                        .on_press(Message::OpenBackgroundFolder)
                        .style(quiet_button),
                    button(text("Refresh List").size(13))
                        .on_press(Message::RefreshBackgrounds)
                        .style(quiet_button),
                    button(text("Browse PolyHaven Backgrounds").size(13))
                        .on_press(Message::DownloadBackgrounds)
                        .style(quiet_button),
                ]
                .spacing(8),
            ]
            .spacing(8),
            self.config.background == defaults.background,
            ResetTarget::BackgroundSection,
        );
        let application = section(
            "APPLICATION",
            labeled_input(
                "LAUNCHER EXECUTABLE",
                &self.config.application.launcher,
                "spacelauncher",
                Message::LauncherChanged,
                self.config.application.launcher == defaults.application.launcher,
                ResetTarget::ApplicationLauncher,
            ),
            self.config.application.launcher == defaults.application.launcher,
            ResetTarget::ApplicationLauncher,
        );

        let floor_height = labeled_input(
            "FALLBACK HEIGHT (M)",
            &self.floor_height,
            "-1.3",
            Message::FloorHeightChanged,
            number_is_default(&self.floor_height, defaults.floor.height_m),
            ResetTarget::FloorHeight,
        );
        let albedo = Color::from_rgba(
            self.config.floor.albedo[0],
            self.config.floor.albedo[1],
            self.config.floor.albedo[2],
            self.config.floor.albedo[3],
        );
        let albedo_picker = color_picker(
            self.show_albedo_picker,
            albedo,
            button(color_swatch(albedo))
                .on_press(Message::ChooseAlbedoColor)
                .style(quiet_button),
            Message::CancelAlbedoColor,
            Message::SubmitAlbedoColor,
        );
        let floor = section(
            "FLOOR MATERIAL",
            column![
                row![
                    floor_height,
                    column![
                        text("ALBEDO (RGBA)").size(11).color(MUTED),
                        row![
                            albedo_picker,
                            reset_button(
                                self.config.floor.albedo == defaults.floor.albedo,
                                ResetTarget::FloorAlbedo,
                            ),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    ]
                    .spacing(8)
                    .width(Length::Fill)
                ]
                .spacing(18)
                .align_y(iced::Alignment::End),
                slider_row(
                    "ROUGHNESS",
                    self.config.floor.roughness,
                    0.0,
                    1.0,
                    0.01,
                    Message::RoughnessChanged,
                    self.config.floor.roughness == defaults.floor.roughness,
                    ResetTarget::FloorRoughness,
                ),
                slider_row(
                    "REFLECTANCE",
                    self.config.floor.reflectance,
                    0.0,
                    1.0,
                    0.01,
                    Message::ReflectanceChanged,
                    self.config.floor.reflectance == defaults.floor.reflectance,
                    ResetTarget::FloorReflectance,
                ),
                row![
                    text("REFLECTION RAYS")
                        .size(11)
                        .color(MUTED)
                        .width(Length::Fixed(126.0)),
                    slider(
                        1..=64,
                        self.config.floor.ray_count,
                        Message::RayCountChanged
                    )
                    .step(1_u32)
                    .width(Length::Fill),
                    text(self.config.floor.ray_count.to_string())
                        .size(13)
                        .color(TEXT)
                        .width(Length::Fixed(34.0))
                        .align_x(Horizontal::Right),
                    reset_button(
                        self.config.floor.ray_count == defaults.floor.ray_count,
                        ResetTarget::FloorRayCount,
                    ),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
                row![
                    checkbox(self.config.floor.trace_through_transparent_windows)
                        .label("Trace reflections through transparent window areas")
                        .on_toggle(Message::TransparentReflectionsChanged)
                        .width(Length::Fill),
                    reset_button(
                        self.config.floor.trace_through_transparent_windows
                            == defaults.floor.trace_through_transparent_windows,
                        ResetTarget::FloorTransparentReflections,
                    ),
                ]
                .align_y(iced::Alignment::Center),
                row![
                    checkbox(self.config.floor.ambient_occlusion)
                        .label("Enable ambient occlusion (experimental)")
                        .on_toggle(Message::AmbientOcclusionChanged)
                        .width(Length::Fill),
                    reset_button(
                        self.config.floor.ambient_occlusion == defaults.floor.ambient_occlusion,
                        ResetTarget::FloorAmbientOcclusion,
                    ),
                ]
                .align_y(iced::Alignment::Center),
                slider_row(
                    "REFLECTION GRID (MM)",
                    self.config.floor.reflection_grain_size_m * 1000.0,
                    1.0,
                    50.0,
                    1.0,
                    Message::ReflectionGrainSizeChanged,
                    self.config.floor.reflection_grain_size_m
                        == defaults.floor.reflection_grain_size_m,
                    ResetTarget::FloorReflectionGrainSize,
                ),
                slider_row(
                    "FLOOR RADIUS (°)",
                    self.config.floor.radius_degrees,
                    0.0,
                    90.0,
                    0.1,
                    Message::GroundRadiusChanged,
                    self.config.floor.radius_degrees == defaults.floor.radius_degrees,
                    ResetTarget::FloorRadius,
                ),
                slider_row(
                    "FEATHERING (M)",
                    self.config.floor.feathering_m,
                    0.0,
                    7.0,
                    0.1,
                    Message::GroundFeatheringChanged,
                    self.config.floor.feathering_m == defaults.floor.feathering_m,
                    ResetTarget::FloorFeathering,
                ),
            ]
            .spacing(18),
            number_is_default(&self.floor_height, defaults.floor.height_m)
                && self.config.floor.albedo == defaults.floor.albedo
                && self.config.floor.roughness == defaults.floor.roughness
                && self.config.floor.reflectance == defaults.floor.reflectance
                && self.config.floor.ray_count == defaults.floor.ray_count
                && self.config.floor.reflection_grain_size_m
                    == defaults.floor.reflection_grain_size_m
                && self.config.floor.radius_degrees == defaults.floor.radius_degrees
                && self.config.floor.feathering_m == defaults.floor.feathering_m
                && self.config.floor.trace_through_transparent_windows
                    == defaults.floor.trace_through_transparent_windows
                && self.config.floor.ambient_occlusion == defaults.floor.ambient_occlusion,
            ResetTarget::FloorSection,
        );

        let window_border_color = Color::from_rgba(
            self.config.window.border_color[0],
            self.config.window.border_color[1],
            self.config.window.border_color[2],
            self.config.window.border_color[3],
        );
        let window_border_picker = color_picker(
            self.show_window_border_picker,
            window_border_color,
            button(color_swatch(window_border_color))
                .on_press(Message::ChooseWindowBorderColor)
                .style(quiet_button),
            Message::CancelWindowBorderColor,
            Message::SubmitWindowBorderColor,
        );
        let cursor_close_border_color = Color::from_rgba(
            self.config.window.cursor_close_border_color[0],
            self.config.window.cursor_close_border_color[1],
            self.config.window.cursor_close_border_color[2],
            self.config.window.cursor_close_border_color[3],
        );
        let cursor_close_border_picker = color_picker(
            self.show_cursor_close_border_picker,
            cursor_close_border_color,
            button(color_swatch(cursor_close_border_color))
                .on_press(Message::ChooseCursorCloseBorderColor)
                .style(quiet_button),
            Message::CancelCursorCloseBorderColor,
            Message::SubmitCursorCloseBorderColor,
        );
        let grabbed_border_color = Color::from_rgba(
            self.config.window.grabbed_border_color[0],
            self.config.window.grabbed_border_color[1],
            self.config.window.grabbed_border_color[2],
            self.config.window.grabbed_border_color[3],
        );
        let grabbed_border_picker = color_picker(
            self.show_grabbed_border_picker,
            grabbed_border_color,
            button(color_swatch(grabbed_border_color))
                .on_press(Message::ChooseGrabbedBorderColor)
                .style(quiet_button),
            Message::CancelGrabbedBorderColor,
            Message::SubmitGrabbedBorderColor,
        );

        let window_scale = section(
            "WINDOW RESOLUTION & AA",
            column![
                labeled_input(
                    "PIXELS PER DEGREE",
                    &self.window_pixels_per_degree,
                    "32",
                    Message::WindowPixelsPerDegreeChanged,
                    number_is_default(
                        &self.window_pixels_per_degree,
                        defaults.window.pixels_per_degree,
                    ),
                    ResetTarget::WindowPixelsPerDegree,
                ),
                row![
                    text("TEXTURE AA MODE").size(11).color(MUTED),
                    pick_list(
                        WindowTextureAa::OPTIONS,
                        Some(self.config.window.texture_aa),
                        Message::WindowTextureAaChanged,
                    )
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
                row![
                    text("REFLECTION TEXTURES").size(11).color(MUTED),
                    pick_list(
                        ReflectionTextures::OPTIONS,
                        Some(self.config.window.reflection_textures),
                        Message::ReflectionTexturesChanged,
                    )
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
                text("Reflection texture mode changes require a Spacetop restart.")
                    .size(11)
                    .color(MUTED),
            ]
            .extend(self.atlas_size_picker())
            .spacing(14),
            number_is_default(
                &self.window_pixels_per_degree,
                defaults.window.pixels_per_degree,
            ) && self.config.window.texture_aa == defaults.window.texture_aa
                && self.config.window.reflection_textures == defaults.window.reflection_textures
                && self.config.window.reflection_atlas_size
                    == defaults.window.reflection_atlas_size,
            ResetTarget::WindowResolutionSection,
        );

        let placement = section(
            "WINDOW PLACEMENT",
            column![
                labeled_input(
                    "DEFAULT DISTANCE (M)",
                    &self.window_distance,
                    "1.6",
                    Message::WindowDistanceChanged,
                    number_is_default(&self.window_distance, defaults.window.default_distance_m),
                    ResetTarget::WindowDistance,
                ),
                labeled_input(
                    "DEFAULT VERTICAL ANGLE (DEG)",
                    &self.window_vertical_angle,
                    "0",
                    Message::WindowVerticalAngleChanged,
                    number_is_default(
                        &self.window_vertical_angle,
                        defaults.window.default_vertical_angle_degrees,
                    ),
                    ResetTarget::WindowVerticalAngle,
                ),
                slider_row(
                    "DODGE HALF-TIME (S)",
                    self.config.window.animation_half_time_s,
                    0.0,
                    5.0,
                    0.05,
                    Message::WindowAnimationHalfTimeChanged,
                    self.config.window.animation_half_time_s
                        == defaults.window.animation_half_time_s,
                    ResetTarget::WindowAnimationHalfTime,
                ),
                slider_row(
                    "DODGE MARGIN (CM)",
                    self.config.window.collision_margin_m * 100.0,
                    0.0,
                    50.0,
                    1.0,
                    Message::WindowCollisionMarginChanged,
                    self.config.window.collision_margin_m == defaults.window.collision_margin_m,
                    ResetTarget::WindowCollisionMargin,
                ),
            ]
            .spacing(10),
            number_is_default(&self.window_distance, defaults.window.default_distance_m)
                && number_is_default(
                    &self.window_vertical_angle,
                    defaults.window.default_vertical_angle_degrees,
                )
                && self.config.window.animation_half_time_s
                    == defaults.window.animation_half_time_s
                && self.config.window.collision_margin_m == defaults.window.collision_margin_m,
            ResetTarget::WindowPlacementSection,
        );

        let windows = section(
            "WINDOWS BORDER",
            column![
                slider_row(
                    "CURSOR PROXIMITY (PX)",
                    self.config.window.cursor_proximity_radius_px,
                    0.0,
                    500.0,
                    1.0,
                    Message::WindowCursorProximityChanged,
                    self.config.window.cursor_proximity_radius_px
                        == defaults.window.cursor_proximity_radius_px,
                    ResetTarget::WindowCursorProximity,
                ),
                iced::widget::rule::horizontal(1),
                slider_row(
                    "PADDING (PX)",
                    self.config.window.padding_px,
                    0.0,
                    500.0,
                    1.0,
                    Message::WindowPaddingChanged,
                    self.config.window.padding_px == defaults.window.padding_px,
                    ResetTarget::WindowPadding,
                ),
                slider_row(
                    "GRAB MARGIN (PX)",
                    self.config.window.margin_px,
                    0.0,
                    500.0,
                    1.0,
                    Message::WindowMarginChanged,
                    self.config.window.margin_px == defaults.window.margin_px,
                    ResetTarget::WindowMargin,
                ),
                slider_row(
                    "BORDER RADIUS (PX)",
                    self.config.window.border_radius_px,
                    0.0,
                    500.0,
                    1.0,
                    Message::WindowBorderRadiusChanged,
                    self.config.window.border_radius_px == defaults.window.border_radius_px,
                    ResetTarget::WindowBorderRadius,
                ),
                iced::widget::rule::horizontal(1),
                slider_row(
                    "BORDER WIDTH (PX)",
                    self.config.window.border_width_px,
                    0.0,
                    100.0,
                    1.0,
                    Message::WindowBorderWidthChanged,
                    self.config.window.border_width_px == defaults.window.border_width_px,
                    ResetTarget::WindowBorderWidth,
                ),
                slider_row(
                    "CLOSE BORDER (PX)",
                    self.config.window.cursor_close_border_width_px,
                    0.0,
                    100.0,
                    1.0,
                    Message::WindowCursorCloseBorderWidthChanged,
                    self.config.window.cursor_close_border_width_px
                        == defaults.window.cursor_close_border_width_px,
                    ResetTarget::WindowCursorCloseBorderWidth,
                ),
                slider_row(
                    "GRABBED BORDER (PX)",
                    self.config.window.grabbed_border_width_px,
                    0.0,
                    100.0,
                    1.0,
                    Message::WindowGrabbedBorderWidthChanged,
                    self.config.window.grabbed_border_width_px
                        == defaults.window.grabbed_border_width_px,
                    ResetTarget::WindowGrabbedBorderWidth,
                ),
                iced::widget::rule::horizontal(1),
                row![
                    column![
                        text("BORDER COLOR").size(11).color(MUTED),
                        row![
                            window_border_picker,
                            reset_button(
                                self.config.window.border_color == defaults.window.border_color,
                                ResetTarget::WindowBorderColor,
                            ),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                    ]
                    .width(Length::Fill)
                    .spacing(8),
                    column![
                        text("CLOSE BORDER COLOR").size(11).color(MUTED),
                        row![
                            cursor_close_border_picker,
                            reset_button(
                                self.config.window.cursor_close_border_color
                                    == defaults.window.cursor_close_border_color,
                                ResetTarget::WindowCursorCloseBorderColor,
                            ),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                    ]
                    .width(Length::Fill)
                    .spacing(8),
                    column![
                        text("GRABBED BORDER COLOR").size(11).color(MUTED),
                        row![
                            grabbed_border_picker,
                            reset_button(
                                self.config.window.grabbed_border_color
                                    == defaults.window.grabbed_border_color,
                                ResetTarget::WindowGrabbedBorderColor,
                            ),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                    ]
                    .width(Length::Fill)
                    .spacing(8),
                ]
                .spacing(16)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(10),
            self.config.window.padding_px == defaults.window.padding_px
                && self.config.window.margin_px == defaults.window.margin_px
                && self.config.window.border_radius_px == defaults.window.border_radius_px
                && self.config.window.border_width_px == defaults.window.border_width_px
                && self.config.window.cursor_proximity_radius_px
                    == defaults.window.cursor_proximity_radius_px
                && self.config.window.cursor_close_border_width_px
                    == defaults.window.cursor_close_border_width_px
                && self.config.window.border_color == defaults.window.border_color
                && self.config.window.cursor_close_border_color
                    == defaults.window.cursor_close_border_color
                && self.config.window.grabbed_border_width_px
                    == defaults.window.grabbed_border_width_px
                && self.config.window.grabbed_border_color == defaults.window.grabbed_border_color,
            ResetTarget::WindowBorderSection,
        );

        let status_color = if self.status_is_error {
            Color::from_rgb(0.98, 0.47, 0.39)
        } else {
            ACCENT
        };
        let footer = row![
            text(self.status.as_str())
                .size(12)
                .color(status_color)
                .width(Length::Fill),
            button(text("Reload").size(13))
                .on_press(Message::Reload)
                .style(quiet_button),
            button(text("Save changes").size(13))
                .on_press(Message::Save)
                .style(accent_button),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        container(
            column![
                row![
                    column![
                        text("SPACE SETTINGS").size(12).color(ACCENT),
                        text("Configuration").size(25).color(TEXT),
                        text("~/.config/spacetop/config.toml").size(12).color(MUTED),
                    ]
                    .spacing(4),
                    iced::widget::Space::new().width(Length::Fill),
                    button(text("Open in Text Editor").size(13))
                        .on_press(Message::OpenConfigFile)
                        .style(quiet_button),
                ]
                .align_y(iced::Alignment::Center),
                scrollable(
                    column![
                        application,
                        environment,
                        floor,
                        placement,
                        window_scale,
                        windows
                    ]
                    .spacing(14),
                )
                .height(Length::Fill),
                footer,
            ]
            .spacing(18)
            .padding(24),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.055, 0.075, 0.085))),
            ..Default::default()
        })
        .into()
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
mod tests {
    use super::*;

    #[test]
    fn background_files_only_includes_regular_exr_files() {
        let directory = tempfile::tempdir().unwrap();
        let exr = directory.path().join("sky.exr");
        fs::write(&exr, []).unwrap();
        fs::write(directory.path().join("preview.png"), []).unwrap();
        fs::create_dir(directory.path().join("nested.exr")).unwrap();

        assert_eq!(background_files_in(directory.path()), vec![exr]);
    }

    #[test]
    fn preview_exposure_brightens_linear_hdr_pixels() {
        let source = image::Rgb32FImage::from_pixel(1, 1, image::Rgb([1.0, 1.0, 1.0]));
        let base = tone_mapped_preview_pixels(&source, 0.0, 0.0);
        let brighter = tone_mapped_preview_pixels(&source, 1.0, 0.0);

        assert!(brighter[0] > base[0]);
        assert_eq!(base[3], 255);
    }

    #[test]
    fn preview_rotation_wraps_the_panorama_horizontally() {
        let source =
            image::Rgb32FImage::from_fn(4, 1, |x, _| image::Rgb([x as f32 / 4.0, 0.0, 0.0]));
        let unrotated = tone_mapped_preview_pixels(&source, 0.0, 0.0);
        let rotated = tone_mapped_preview_pixels(&source, 0.0, 90.0);

        assert_eq!(rotated[0], unrotated[4]);
        assert_eq!(rotated[12], unrotated[0]);
    }

    #[test]
    fn atlas_size_dropdown_is_only_visible_for_explicit_atlas_mode() {
        let mut app = SettingsApp::from_config(AppConfig::default(), String::new(), false);
        for mode in ReflectionTextures::OPTIONS {
            app.config.window.reflection_textures = mode;
            assert_eq!(
                app.atlas_size_picker().is_some(),
                mode == ReflectionTextures::Atlas
            );
        }
        let _ = app.update(Message::ReflectionAtlasSizeChanged(
            ReflectionAtlasSize::Size2048,
        ));
        assert_eq!(
            app.config.window.reflection_atlas_size,
            ReflectionAtlasSize::Size2048
        );
    }

    #[test]
    fn ambient_occlusion_setting_can_be_toggled_and_reset() {
        let mut app = SettingsApp::from_config(AppConfig::default(), String::new(), false);

        let _ = app.update(Message::AmbientOcclusionChanged(true));
        assert!(app.config.floor.ambient_occlusion);

        app.reset(ResetTarget::FloorAmbientOcclusion);
        assert!(!app.config.floor.ambient_occlusion);
    }

    #[test]
    fn section_resets_restore_defaults_and_text_fields() {
        let defaults = AppConfig::default();
        let mut config = defaults.clone();
        config.background.image = "custom.exr".into();
        config.application.launcher = "custom-launcher".into();
        config.background.brightness_stops = 1.0;
        config.floor.height_m = -2.0;
        config.floor.roughness = 0.8;
        config.floor.ambient_occlusion = true;
        config.window.default_distance_m = 2.0;
        config.window.default_vertical_angle_degrees = 10.0;
        config.window.pixels_per_degree = 40.0;
        config.window.reflection_textures = ReflectionTextures::Atlas;
        config.window.reflection_atlas_size = ReflectionAtlasSize::Size4096;
        config.window.border_width_px = 5.0;

        let mut app = SettingsApp::from_config(config, String::new(), false);
        app.reset(ResetTarget::ApplicationLauncher);
        app.reset(ResetTarget::BackgroundSection);
        app.reset(ResetTarget::FloorSection);
        app.reset(ResetTarget::WindowPlacementSection);
        app.reset(ResetTarget::WindowResolutionSection);
        app.reset(ResetTarget::WindowBorderSection);

        assert_eq!(app.config, defaults);
        assert_eq!(app.floor_height, defaults.floor.height_m.to_string());
        assert_eq!(
            app.window_distance,
            defaults.window.default_distance_m.to_string()
        );
        assert_eq!(
            app.window_vertical_angle,
            defaults.window.default_vertical_angle_degrees.to_string()
        );
        assert_eq!(
            app.window_pixels_per_degree,
            defaults.window.pixels_per_degree.to_string()
        );
    }
}
