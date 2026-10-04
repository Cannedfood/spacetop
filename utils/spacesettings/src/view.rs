use super::*;
use iced::widget::column;

impl SettingsApp {
    pub(super) fn view(&self) -> Element<'_, Message> {
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
                        .menu_height(Length::Fixed(
                            (self.backgrounds.len().saturating_add(1) as f32 * 38.0).min(280.0),
                        ))
                        .padding([9, 20])
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
                    "DENSITY PIXELS PER DEGREE",
                    &self.window_pixels_per_degree,
                    "30",
                    Message::WindowPixelsPerDegreeChanged,
                    number_is_default(
                        &self.window_pixels_per_degree,
                        defaults.window.pixels_per_degree,
                    ),
                    ResetTarget::WindowPixelsPerDegree,
                ),
                slider_row(
                    "DISPLAY SCALE",
                    self.window_display_scale,
                    0.5,
                    4.0,
                    0.5,
                    Message::WindowDisplayScaleChanged,
                    self.window_display_scale == defaults.window.display_scale,
                    ResetTarget::WindowDisplayScale,
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
            ]
            .push(self.atlas_size_picker())
            .spacing(14),
            number_is_default(
                &self.window_pixels_per_degree,
                defaults.window.pixels_per_degree,
            ) && self.window_display_scale == defaults.window.display_scale
                && self.config.window.texture_aa == defaults.window.texture_aa
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
        let fullscreen = section(
            "FULLSCREEN",
            column![
                slider_row(
                    "MAX WIDTH (DEG)",
                    self.config.window.fullscreen_max_width_degrees,
                    1.0,
                    170.0,
                    1.0,
                    Message::WindowFullscreenMaxWidthChanged,
                    self.config.window.fullscreen_max_width_degrees
                        == defaults.window.fullscreen_max_width_degrees,
                    ResetTarget::WindowFullscreenSection,
                ),
                slider_row(
                    "MAX HEIGHT (DEG)",
                    self.config.window.fullscreen_max_height_degrees,
                    1.0,
                    170.0,
                    1.0,
                    Message::WindowFullscreenMaxHeightChanged,
                    self.config.window.fullscreen_max_height_degrees
                        == defaults.window.fullscreen_max_height_degrees,
                    ResetTarget::WindowFullscreenSection,
                ),
                slider_row(
                    "ENVIRONMENT DIMMING (%)",
                    self.config.window.fullscreen_environment_dim * 100.0,
                    0.0,
                    100.0,
                    1.0,
                    Message::WindowFullscreenEnvironmentDimChanged,
                    self.config.window.fullscreen_environment_dim
                        == defaults.window.fullscreen_environment_dim,
                    ResetTarget::WindowFullscreenSection,
                ),
            ]
            .spacing(10),
            self.config.window.fullscreen_max_width_degrees
                == defaults.window.fullscreen_max_width_degrees
                && self.config.window.fullscreen_max_height_degrees
                    == defaults.window.fullscreen_max_height_degrees
                && self.config.window.fullscreen_environment_dim
                    == defaults.window.fullscreen_environment_dim,
            ResetTarget::WindowFullscreenSection,
        );
        let maximized = section(
            "MAXIMIZED WINDOW",
            column![
                slider_row(
                    "MAX WIDTH (DEG)",
                    self.config.window.maximized_max_width_degrees,
                    1.0,
                    170.0,
                    1.0,
                    Message::WindowMaximizedMaxWidthChanged,
                    self.config.window.maximized_max_width_degrees
                        == defaults.window.maximized_max_width_degrees,
                    ResetTarget::WindowMaximizedSection,
                ),
                slider_row(
                    "MAX HEIGHT (DEG)",
                    self.config.window.maximized_max_height_degrees,
                    1.0,
                    170.0,
                    1.0,
                    Message::WindowMaximizedMaxHeightChanged,
                    self.config.window.maximized_max_height_degrees
                        == defaults.window.maximized_max_height_degrees,
                    ResetTarget::WindowMaximizedSection,
                ),
            ]
            .spacing(10),
            self.config.window.maximized_max_width_degrees
                == defaults.window.maximized_max_width_degrees
                && self.config.window.maximized_max_height_degrees
                    == defaults.window.maximized_max_height_degrees,
            ResetTarget::WindowMaximizedSection,
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
                container(
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
                )
                .padding([20, 24]),
                scrollable(
                    column![
                        application,
                        environment,
                        floor,
                        placement,
                        fullscreen,
                        maximized,
                        window_scale,
                        windows
                    ]
                    .spacing(14)
                    .padding([8, 24]),
                )
                .height(Length::Fill),
                container(footer).padding([16, 24]),
            ]
            .spacing(0),
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
