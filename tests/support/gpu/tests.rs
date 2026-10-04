use super::*;

#[test]
#[ignore = "requires a Vulkan/GLES GPU with DMA-BUF sharing"]
fn vulkan_reflection_atlas_renders_reflections() -> Result<()> {
    render_reflection_atlas()
}

fn render_reflection_atlas() -> Result<()> {
    let vulkan = Vulkan::new()?;
    let mut config = crate::config::AppConfig::default();
    config.floor.albedo = [0.0, 0.0, 0.0, 1.0];
    config.window.padding_px = 0.0;
    config.window.border_radius_px = 0.0;
    config.window.border_width_px = 0.0;
    config.window.cursor_close_border_width_px = 0.0;
    config.window.grabbed_border_width_px = 0.0;
    let mut scene = SceneRenderer::new(
        &vulkan.device,
        &vulkan.instance,
        vulkan.physical_device,
        vk::Format::R8G8B8A8_SRGB,
        &config,
    )?;
    let directory = tempfile::tempdir()?;
    let skybox_path = directory.path().join("black.exr");
    image::Rgb32FImage::from_pixel(8, 4, image::Rgb([0.0, 0.0, 0.0])).save(&skybox_path)?;
    let mut skybox = SkyboxTexture::new(
        &scene,
        &vulkan.instance,
        vulkan.physical_device,
        skybox_path.to_str().unwrap(),
    )?;
    let mut producer = GpuRenderer::new(&vulkan.render_node)?;
    let mut make_texture = |size: u32, color: [f32; 4]| -> Result<PanelTexture> {
        let buffer =
            producer
                .allocator
                .create_buffer(size, size, Fourcc::Abgr8888, &[Modifier::Linear])?;
        let mut dmabuf = buffer.export()?;
        {
            let mut target = producer.renderer.bind(&mut dmabuf)?;
            let size = (size as i32, size as i32).into();
            let mut frame = producer
                .renderer
                .render(&mut target, size, Transform::Normal)?;
            frame.clear(color.into(), &[smithay::utils::Rectangle::from_size(size)])?;
            if size.w <= 64 {
                frame.clear(
                    [0.0, 0.0, 0.25, 0.5].into(),
                    &[smithay::utils::Rectangle::from_size(
                        (size.w / 2, size.h / 2).into(),
                    )],
                )?;
            }
            frame.finish()?.wait()?;
        }
        PanelTexture::new(
            &scene,
            SharedImage::import(
                &vulkan.instance,
                &vulkan.device,
                vulkan.physical_device,
                dmabuf,
            )?,
        )
    };
    let red = make_texture(32, [0.5, 0.0, 0.0, 0.5])?;
    let white = make_texture(64, [1.0, 1.0, 1.0, 1.0])?;
    let green = make_texture(16, [0.0, 0.75, 0.0, 1.0])?;
    let oversized = make_texture(2048, [1.0, 1.0, 1.0, 1.0])?;
    let geometry = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -3.0),
            yaw: 0.0,
            pitch: 0.0,
            width_m: 1.0,
        },
        logical_size: (32, 32).into(),
    };
    let farther = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -4.0),
            width_m: 4.0 / 3.0,
            ..geometry.pose
        },
        ..geometry
    };
    let view = openxr::View {
        pose: openxr::Posef::IDENTITY,
        fov: openxr::Fovf {
            angle_left: -std::f32::consts::FRAC_PI_4,
            angle_right: std::f32::consts::FRAC_PI_4,
            angle_up: std::f32::consts::FRAC_PI_4,
            angle_down: -std::f32::consts::FRAC_PI_4,
        },
    };
    for transparent in [false, true] {
        config.floor.trace_through_transparent_windows = transparent;
        config.window.reflection_atlas_size = if transparent {
            spacetop_config::ReflectionAtlasSize::Size256
        } else {
            spacetop_config::ReflectionAtlasSize::Size1024
        };
        scene.update_config(&config)?;
        for panels in [
            vec![],
            vec![(&red, geometry)],
            vec![(&red, geometry), (&white, farther)],
            vec![(&white, farther), (&red, geometry), (&green, farther)],
            vec![(&green, geometry)],
            vec![(&oversized, geometry)],
            vec![],
        ] {
            let mut render = |scene: &mut SceneRenderer| {
                vulkan.readback_image(
                    None,
                    (512, 512).into(),
                    None,
                    Some(SceneReadback {
                        renderer: scene,
                        view: &view,
                        skybox: Some(&mut skybox),
                        panels: &panels,
                        cursor: None,
                        floor_y: config.floor.height_m,
                    }),
                )
            };
            let actual = render(&mut scene)?;
            if !panels.is_empty() {
                let reflected = (430..512)
                    .flat_map(|y| (0..512).map(move |x| (y * 512 + x) * 4))
                    .filter(|&offset| actual[offset..offset + 3].iter().any(|&value| value != 0))
                    .count();
                assert!(
                    reflected > 100,
                    "atlas must contain actual floor reflections"
                );
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires a Vulkan/GLES GPU with DMA-BUF sharing"]
fn vulkan_scene_renders_sampled_panels_and_cursor() -> Result<()> {
    let vulkan = Vulkan::new()?;
    let mut window_test_config = crate::config::AppConfig::default();
    window_test_config.floor.albedo = [0.0, 0.0, 0.0, 1.0];
    window_test_config.window.padding_px = 0.0;
    window_test_config.window.border_radius_px = 0.0;
    window_test_config.window.border_width_px = 0.0;
    window_test_config.window.cursor_close_border_width_px = 0.0;
    window_test_config.window.grabbed_border_width_px = 0.0;
    let mut scene = SceneRenderer::new(
        &vulkan.device,
        &vulkan.instance,
        vulkan.physical_device,
        vk::Format::R8G8B8A8_SRGB,
        &window_test_config,
    )?;
    let skybox_directory = tempfile::tempdir()?;
    let skybox_path = skybox_directory.path().join("black.exr");
    image::Rgb32FImage::from_pixel(8, 4, image::Rgb([0.0, 0.0, 0.0])).save(&skybox_path)?;
    let skybox_path = skybox_path.to_string_lossy().into_owned();
    let mut skybox = SkyboxTexture::new(
        &scene,
        &vulkan.instance,
        vulkan.physical_device,
        &skybox_path,
    )?;
    let mut producer = GpuRenderer::new(&vulkan.render_node)?;
    let mut make_texture = |quadrants: bool| -> Result<PanelTexture> {
        let buffer =
            producer
                .allocator
                .create_buffer(32, 32, Fourcc::Abgr8888, &[Modifier::Linear])?;
        let mut dmabuf = buffer.export()?;
        {
            let mut framebuffer = producer.renderer.bind(&mut dmabuf)?;
            let mut frame =
                producer
                    .renderer
                    .render(&mut framebuffer, (32, 32).into(), Transform::Normal)?;
            let full = [smithay::utils::Rectangle::from_size((32, 32).into())];
            frame.clear(
                smithay::backend::renderer::Color32F::new(1.0, 1.0, 1.0, 1.0),
                &full,
            )?;
            if quadrants {
                for (location, color) in [
                    ((0, 0), [1.0, 0.0, 0.0, 1.0]),
                    ((16, 0), [0.0, 1.0, 0.0, 1.0]),
                    ((0, 16), [0.0, 0.0, 1.0, 1.0]),
                    ((16, 16), [0.0, 0.0, 0.0, 0.0]),
                ] {
                    frame.clear(
                        color.into(),
                        &[smithay::utils::Rectangle::new(
                            location.into(),
                            (16, 16).into(),
                        )],
                    )?;
                }
            }
            frame.finish()?.wait()?;
        }
        PanelTexture::new(
            &scene,
            SharedImage::import(
                &vulkan.instance,
                &vulkan.device,
                vulkan.physical_device,
                dmabuf,
            )?,
        )
    };
    let foreground = make_texture(true)?;
    let background = make_texture(false)?;
    let second_background = make_texture(false)?;
    let near = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -1.0),
            yaw: 0.0,
            pitch: 0.0,
            width_m: 1.0,
        },
        logical_size: (32, 32).into(),
    };
    let far = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -2.0),
            width_m: 2.0,
            ..near.pose
        },
        ..near
    };
    let panels = [(&foreground, near), (&background, far)];
    let mut view = openxr::View {
        pose: openxr::Posef::IDENTITY,
        fov: openxr::Fovf {
            angle_left: -std::f32::consts::FRAC_PI_4,
            angle_right: std::f32::consts::FRAC_PI_4,
            angle_up: std::f32::consts::FRAC_PI_4,
            angle_down: -std::f32::consts::FRAC_PI_4,
        },
    };
    let cursor = PanelPose {
        width_m: 0.021,
        ..near.pose
    };
    let pixels = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &panels,
            cursor: Some(cursor),
            floor_y: default_floor_height(),
        }),
    )?;
    let pixel = |pixels: &[u8], horizontal: usize, vertical: usize| {
        pixels[(vertical * 512 + horizontal) * 4..][..4].to_vec()
    };
    assert_eq!(pixel(&pixels, 200, 200), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 300, 200), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, 200, 300), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, 300, 300), [255, 255, 255, 255]);
    assert_eq!(pixel(&pixels, 30, 30), [0, 0, 0, 255]);
    let mut padded_window_config = window_test_config.clone();
    padded_window_config.window.padding_px = 12.0;
    scene.update_config(&padded_window_config)?;
    let padded_window_pixels = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &panels,
            cursor: Some(cursor),
            floor_y: default_floor_height(),
        }),
    )?;
    assert_eq!(
        pixels, padded_window_pixels,
        "padding must expand around content without changing its physical size"
    );
    scene.update_config(&window_test_config)?;
    let single_panel_reference = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[(&background, near)],
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    let mut translucent_border_config = window_test_config.clone();
    translucent_border_config.window.border_width_px = 2.0;
    translucent_border_config.window.border_color = [1.0, 0.0, 0.0, 0.5];
    scene.update_config(&translucent_border_config)?;
    let translucent_pixels = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[(&background, near)],
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    let outer_translucent_border = (0..512)
        .flat_map(|vertical| (0..512).map(move |horizontal| (horizontal, vertical)))
        .find(|(horizontal, vertical)| {
            pixel(&single_panel_reference, *horizontal, *vertical) == [0, 0, 0, 255]
                && (180..=195).contains(&pixel(&translucent_pixels, *horizontal, *vertical)[0])
                && pixel(&translucent_pixels, *horizontal, *vertical)[1..3] == [0, 0]
        });
    assert!(
        outer_translucent_border.is_some(),
        "half-alpha red border should extend and blend outside the original window bounds"
    );
    scene.update_config(&window_test_config)?;
    let cross = pixel(&pixels, 255, 255);
    assert_eq!(cross[0], 255);
    assert!(cross[1].abs_diff(245) <= 1);
    assert_eq!(&cross[2..], [0, 255]);
    assert_eq!(pixel(&pixels, 380, 200), [0, 255, 0, 255]);
    view.pose.position.x = 0.1;
    let moved = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &panels,
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    assert_eq!(pixel(&moved, 380, 200), [0, 0, 0, 255]);
    assert_eq!(pixel(&moved, 200, 200), [255, 0, 0, 255]);
    view.pose = openxr::Posef::IDENTITY;
    let emitter = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -3.0),
            ..near.pose
        },
        ..near
    };
    let floor_pixels = |scene: &mut SceneRenderer,
                        skybox: &mut SkyboxTexture,
                        panels: &[(&PanelTexture, PanelGeometry)]| {
        vulkan.readback_image(
            None,
            (512, 512).into(),
            None,
            Some(SceneReadback {
                renderer: scene,
                view: &view,
                skybox: Some(skybox),
                panels,
                cursor: None,
                floor_y: default_floor_height(),
            }),
        )
    };
    let empty = floor_pixels(&mut scene, &mut skybox, &[])?;
    let empty_floor_pixel = pixel(&empty, 256, 450);
    assert_eq!(empty_floor_pixel, [0, 0, 0, 255]);
    let lit = floor_pixels(&mut scene, &mut skybox, &[(&background, emitter)])?;
    let reflection_pixel_count = |pixels: &[u8]| {
        (430..512)
            .flat_map(|vertical| (0..512).map(move |horizontal| (horizontal, vertical)))
            .filter(|(horizontal, vertical)| {
                pixel(pixels, *horizontal, *vertical) != pixel(&empty, *horizontal, *vertical)
            })
            .count()
    };
    let full_content_reflections = reflection_pixel_count(&lit);
    let mut padded_config = window_test_config.clone();
    padded_config.window.padding_px = 12.0;
    scene.update_config(&padded_config)?;
    let padded_lit = floor_pixels(&mut scene, &mut skybox, &[(&background, emitter)])?;
    assert_eq!(
        reflection_pixel_count(&padded_lit),
        full_content_reflections,
        "padding must not shrink or occlude reflected content"
    );
    assert_eq!(
        lit, padded_lit,
        "padding must not alter content reflections"
    );
    scene.update_config(&window_test_config)?;
    let mut brighter_sky_config = window_test_config.clone();
    brighter_sky_config.background.brightness_stops = 2.0;
    scene.update_config(&brighter_sky_config)?;
    let unchanged_reflection = floor_pixels(&mut scene, &mut skybox, &[(&background, emitter)])?;
    assert_eq!(
        lit, unchanged_reflection,
        "skybox brightness must not alter window reflections"
    );
    scene.update_config(&window_test_config)?;
    let repeated = floor_pixels(&mut scene, &mut skybox, &[(&background, emitter)])?;
    assert_eq!(lit, repeated);
    let mut shifted_view = view;
    shifted_view.fov.angle_left = (-1.0_f32 - 16.0 / 256.0).atan();
    shifted_view.fov.angle_right = (1.0_f32 - 16.0 / 256.0).atan();
    let shifted = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &shifted_view,
            skybox: Some(&mut skybox),
            panels: &[(&background, emitter)],
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    let mut anchored_pixels = 0;
    for vertical in 430..470 {
        for horizontal in 240..272 {
            if pixel(&lit, horizontal, vertical) == pixel(&shifted, horizontal + 16, vertical) {
                anchored_pixels += 1;
            }
        }
    }
    assert!(
        anchored_pixels >= 1250,
        "reflection noise must follow floor positions when their screen coordinates change"
    );
    let mut isolated_variations = 0;
    for vertical in 430..470 {
        for horizontal in 240..272 {
            let left = i32::from(pixel(&lit, horizontal - 1, vertical)[0]);
            let middle = i32::from(pixel(&lit, horizontal, vertical)[0]);
            let right = i32::from(pixel(&lit, horizontal + 1, vertical)[0]);
            if (middle > left && middle > right) || (middle < left && middle < right) {
                isolated_variations += 1;
            }
        }
    }
    assert!(
        isolated_variations > 100,
        "GGX samples must vary per pixel, not form coherent repeated reflections"
    );
    let doubled = floor_pixels(
        &mut scene,
        &mut skybox,
        &[(&background, emitter), (&second_background, emitter)],
    )?;
    let colored = floor_pixels(&mut scene, &mut skybox, &[(&foreground, emitter)])?;
    let backwards = floor_pixels(
        &mut scene,
        &mut skybox,
        &[(
            &background,
            PanelGeometry {
                pose: PanelPose {
                    yaw: std::f32::consts::PI,
                    ..emitter.pose
                },
                ..emitter
            },
        )],
    )?;
    let floor_pixel = pixel(&lit, 256, 450);
    assert!(floor_pixel[0] > 0);
    assert!(floor_pixel[0] > empty_floor_pixel[0]);
    assert_eq!(floor_pixel[0], floor_pixel[1]);
    assert_eq!(floor_pixel[1], floor_pixel[2]);
    assert_eq!(floor_pixel[3], 255);
    assert_eq!(pixel(&backwards, 256, 450), empty_floor_pixel);
    let twice = pixel(&doubled, 256, 450);
    assert_eq!(
        twice, floor_pixel,
        "an opaque nearest window should hide an identical farther hit"
    );
    assert_eq!(twice[3], 255);
    let colored_pixel = pixel(&colored, 256, 450);
    assert!(colored_pixel[..3].iter().any(|channel| *channel > 0));
    assert!(
        colored_pixel[..3]
            .iter()
            .zip(&floor_pixel[..3])
            .all(|(colored, white)| colored <= white)
    );
    assert_ne!(colored_pixel, floor_pixel);
    let lowered = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[(&background, emitter)],
            cursor: None,
            floor_y: -2.6,
        }),
    )?;
    assert_eq!(pixel(&lowered, 256, 450), empty_floor_pixel);
    let mut stage_view = view;
    stage_view.pose.position.y = -default_floor_height();
    let stage_emitter = PanelGeometry {
        pose: PanelPose {
            center: emitter.pose.center - glam::Vec3::Y * default_floor_height(),
            ..emitter.pose
        },
        ..emitter
    };
    let calibrated = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &mut scene,
            view: &stage_view,
            skybox: Some(&mut skybox),
            panels: &[(&background, stage_emitter)],
            cursor: None,
            floor_y: 0.0,
        }),
    )?;
    assert_eq!(pixel(&calibrated, 256, 450), floor_pixel);
    assert_eq!(pixel(&calibrated, 256, 256), pixel(&lit, 256, 256));
    Ok(())
}

#[test]
#[ignore = "requires Vulkan DMA-BUF support and a configured EXR skybox"]
fn vulkan_scene_renders_equirectangular_skybox() -> Result<()> {
    let vulkan = Vulkan::new()?;
    let mut renderer = SceneRenderer::new(
        &vulkan.device,
        &vulkan.instance,
        vulkan.physical_device,
        vk::Format::R8G8B8A8_SRGB,
        &crate::config::AppConfig::default(),
    )?;
    let mut skybox = SkyboxTexture::new(
        &renderer,
        &vulkan.instance,
        vulkan.physical_device,
        "random",
    )?;
    let view = openxr::View {
        pose: openxr::Posef::IDENTITY,
        fov: openxr::Fovf {
            angle_left: -std::f32::consts::FRAC_PI_4,
            angle_right: std::f32::consts::FRAC_PI_4,
            angle_up: std::f32::consts::FRAC_PI_4,
            angle_down: -std::f32::consts::FRAC_PI_4,
        },
    };
    let pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &mut renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255)
    );
    let colors = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect::<std::collections::HashSet<_>>();
    assert!(colors.len() > 8, "skybox should vary across the view");
    let ground_colors = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(index, _)| index / 128 >= 56)
        .map(|(_, pixel)| [pixel[0], pixel[1], pixel[2]])
        .collect::<std::collections::HashSet<_>>();
    assert!(
        ground_colors.len() > 1,
        "transparent floor should reveal variations in the skybox"
    );

    let brightness_sum = |pixels: &[u8]| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| u64::from(pixel[0]) + u64::from(pixel[1]) + u64::from(pixel[2]))
            .sum::<u64>()
    };
    let base_brightness = brightness_sum(&pixels);
    let mut brighter_config = crate::config::AppConfig::default();
    brighter_config.background.brightness_stops = 1.0;
    renderer.update_config(&brighter_config)?;
    let brighter_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &mut renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    assert!(
        brightness_sum(&brighter_pixels) > base_brightness,
        "increasing skybox exposure by one stop should brighten the rendered image"
    );
    let mut rotated_config = crate::config::AppConfig::default();
    rotated_config.background.rotation_degrees = 90.0;
    renderer.update_config(&rotated_config)?;
    let rotated_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &mut renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    assert_ne!(
        pixels, rotated_pixels,
        "skybox rotation should change the view"
    );
    let mut reflective_floor_config = crate::config::AppConfig::default();
    reflective_floor_config.floor.albedo = [0.0, 0.0, 0.0, 1.0];
    reflective_floor_config.floor.reflectance = 1.0;
    reflective_floor_config.floor.roughness = 0.0;
    renderer.update_config(&reflective_floor_config)?;
    let reflected_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &mut renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: default_floor_height(),
        }),
    )?;
    let sky_only_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &mut renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: f32::NAN,
        }),
    )?;
    let ground_start = 56 * 128 * 4;
    assert_ne!(
        &reflected_pixels[ground_start..],
        &sky_only_pixels[ground_start..],
        "ground should show the skybox reflected across its surface"
    );
    Ok(())
}

#[test]
#[ignore = "requires Vulkan DMA-BUF support and a configured EXR skybox"]
fn vulkan_floor_fresnel_dims_albedo_at_grazing_angles() -> Result<()> {
    let vulkan = Vulkan::new()?;
    let mut config = crate::config::AppConfig::default();
    config.floor.albedo = [0.2, 0.3, 0.4, 0.6];
    config.floor.reflectance = 0.12;
    let floor_config = &config.floor;
    let mut renderer = SceneRenderer::new(
        &vulkan.device,
        &vulkan.instance,
        vulkan.physical_device,
        vk::Format::R8G8B8A8_SRGB,
        &crate::config::AppConfig::default(),
    )?;
    renderer.update_config(&config)?;
    let skybox_directory = tempfile::tempdir()?;
    let skybox_path = skybox_directory.path().join("black.exr");
    image::Rgb32FImage::from_pixel(8, 4, image::Rgb([0.0, 0.0, 0.0])).save(&skybox_path)?;
    let skybox_path = skybox_path.to_string_lossy().into_owned();
    let mut skybox = SkyboxTexture::new(
        &renderer,
        &vulkan.instance,
        vulkan.physical_device,
        &skybox_path,
    )?;
    let view_at = |target: glam::Vec3, eye: glam::Vec3| {
        let orientation =
            glam::Quat::from_rotation_arc(glam::Vec3::NEG_Z, (target - eye).normalize());
        let mut view = openxr::View {
            pose: openxr::Posef::IDENTITY,
            fov: openxr::Fovf {
                angle_left: -std::f32::consts::FRAC_PI_4,
                angle_right: std::f32::consts::FRAC_PI_4,
                angle_up: std::f32::consts::FRAC_PI_4,
                angle_down: -std::f32::consts::FRAC_PI_4,
            },
        };
        view.pose.position.x = eye.x;
        view.pose.position.y = eye.y;
        view.pose.position.z = eye.z;
        view.pose.orientation.x = orientation.x;
        view.pose.orientation.y = orientation.y;
        view.pose.orientation.z = orientation.z;
        view.pose.orientation.w = orientation.w;
        view
    };
    let mut render = |view: &openxr::View, floor_y: f32| {
        vulkan.readback_image(
            None,
            (128, 128).into(),
            None,
            Some(SceneReadback {
                renderer: &mut renderer,
                view,
                skybox: Some(&mut skybox),
                panels: &[],
                cursor: None,
                floor_y,
            }),
        )
    };
    let head_target = glam::Vec3::new(0.0, default_floor_height(), -2.0);
    let head_view = view_at(head_target, glam::Vec3::new(0.0, 0.0, -2.0));
    let grazing_view = view_at(head_target, glam::Vec3::new(5.0, 0.0, -2.0));
    let head_on = render(&head_view, default_floor_height())?;
    let head_sky = render(&head_view, f32::NAN)?;
    let grazing = render(&grazing_view, default_floor_height())?;
    let grazing_sky = render(&grazing_view, f32::NAN)?;
    let center = (64 * 128 + 64) * 4;
    let decode = |channel: u8| {
        let encoded = f32::from(channel) / 255.0;
        if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        }
    };
    let recovered_albedo = |floor: &[u8], sky: &[u8]| {
        (decode(floor[center]) - (1.0 - floor_config.albedo[3]) * decode(sky[center]))
            / floor_config.albedo[3]
    };
    let head_albedo = recovered_albedo(&head_on, &head_sky);
    let expected_head_albedo = floor_config.albedo[0] * (1.0 - floor_config.reflectance);
    assert!(
        (head_albedo - expected_head_albedo).abs() < 0.02,
        "runtime floor settings should reach the shader: expected={expected_head_albedo}, actual={head_albedo}"
    );
    assert!(
        head_albedo > recovered_albedo(&grazing, &grazing_sky),
        "floor albedo should dim at grazing angles: head-on={}, grazing={}",
        head_albedo,
        recovered_albedo(&grazing, &grazing_sky)
    );
    let distant_target = glam::Vec3::new(0.0, default_floor_height(), -45.0);
    let distant_view = view_at(distant_target, glam::Vec3::ZERO);
    let distant_floor = render(&distant_view, default_floor_height())?;
    let distant_sky = render(&distant_view, f32::NAN)?;
    assert_ne!(
        &distant_floor[center..center + 3],
        &distant_sky[center..center + 3],
        "ground shading should extend beyond the old 60m floor quad"
    );
    Ok(())
}
