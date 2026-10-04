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
    fn atlas_size_dropdown_is_always_available() {
        let mut app = SettingsApp::from_config(AppConfig::default(), String::new(), false);
        let picker = app.atlas_size_picker();
        drop(picker);
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
    fn fullscreen_settings_can_be_changed_and_reset() {
        let mut app = SettingsApp::from_config(AppConfig::default(), String::new(), false);

        let _ = app.update(Message::WindowFullscreenMaxWidthChanged(120.0));
        let _ = app.update(Message::WindowFullscreenMaxHeightChanged(80.0));
        let _ = app.update(Message::WindowFullscreenEnvironmentDimChanged(75.0));
        assert_eq!(app.config.window.fullscreen_max_width_degrees, 120.0);
        assert_eq!(app.config.window.fullscreen_max_height_degrees, 80.0);
        assert_eq!(app.config.window.fullscreen_environment_dim, 0.75);

        app.reset(ResetTarget::WindowFullscreenSection);
        assert_eq!(app.config.window, AppConfig::default().window);
    }

    #[test]
    fn maximized_window_settings_can_be_changed_and_reset() {
        let mut app = SettingsApp::from_config(AppConfig::default(), String::new(), false);

        let _ = app.update(Message::WindowMaximizedMaxWidthChanged(90.0));
        let _ = app.update(Message::WindowMaximizedMaxHeightChanged(60.0));
        assert_eq!(app.config.window.maximized_max_width_degrees, 90.0);
        assert_eq!(app.config.window.maximized_max_height_degrees, 60.0);

        app.reset(ResetTarget::WindowMaximizedSection);
        assert_eq!(
            app.config.window.maximized_max_width_degrees,
            AppConfig::default().window.maximized_max_width_degrees
        );
        assert_eq!(
            app.config.window.maximized_max_height_degrees,
            AppConfig::default().window.maximized_max_height_degrees
        );
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
        config.window.display_scale = 2.0;
        config.window.reflection_atlas_size = ReflectionAtlasSize::Size4096;
        config.window.border_width_px = 5.0;
        config.window.fullscreen_max_width_degrees = 120.0;
        config.window.fullscreen_max_height_degrees = 80.0;
        config.window.fullscreen_environment_dim = 0.75;
        config.window.maximized_max_width_degrees = 90.0;
        config.window.maximized_max_height_degrees = 60.0;

        let mut app = SettingsApp::from_config(config, String::new(), false);
        app.reset(ResetTarget::ApplicationLauncher);
        app.reset(ResetTarget::BackgroundSection);
        app.reset(ResetTarget::FloorSection);
        app.reset(ResetTarget::WindowPlacementSection);
        app.reset(ResetTarget::WindowFullscreenSection);
        app.reset(ResetTarget::WindowMaximizedSection);
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
        assert_eq!(app.window_display_scale, defaults.window.display_scale);
    }
