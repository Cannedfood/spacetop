use super::*;

#[test]
fn reflection_atlas_packs_native_sizes_without_overlap() {
    for sizes in [
        vec![],
        vec![vk::Extent2D {
            width: 1,
            height: 1,
        }],
        vec![
            vk::Extent2D {
                width: 5,
                height: 2,
            },
            vk::Extent2D {
                width: 3,
                height: 8,
            },
            vk::Extent2D {
                width: 7,
                height: 4,
            },
            vk::Extent2D {
                width: 1,
                height: 1,
            },
        ],
        vec![
            vk::Extent2D {
                width: 8,
                height: 8
            };
            4
        ],
    ] {
        let (extent, rects) = pack_reflection_atlas(&sizes, 16).unwrap();
        assert_eq!(extent.width, 16);
        assert_eq!(extent.height, 16);
        assert_eq!(rects.len(), sizes.len());
        let mut occupied = std::collections::HashSet::new();
        for (size, rect) in sizes.iter().zip(rects) {
            assert!(size == &rect.extent);
            for y in rect.offset.y as u32..rect.offset.y as u32 + size.height {
                for x in rect.offset.x as u32..rect.offset.x as u32 + size.width {
                    assert!(x < extent.width && y < extent.height);
                    assert!(occupied.insert((x, y)));
                }
            }
        }
    }
}

#[test]
fn reflection_atlas_rejects_invalid_sizes_and_capacity_overflow() {
    for size in [
        vk::Extent2D {
            width: 0,
            height: 1,
        },
        vk::Extent2D {
            width: 1,
            height: 0,
        },
    ] {
        assert!(pack_reflection_atlas(&[size], 16).is_err());
    }
    assert!(
        pack_reflection_atlas(
            &[vk::Extent2D {
                width: 1,
                height: 1
            }; 257],
            16
        )
        .is_err()
    );
    assert!(pack_reflection_atlas(&[], 0).is_err());
}

#[test]
fn atlas_only_marks_redrawn_windows_dirty_unless_layout_changes() {
    let first = PanelTextureId(10);
    let second = PanelTextureId(20);
    let changed = PanelTextureId(21);
    assert_eq!(
        atlas_dirty_indices(&[first, second], &[first, second], false, false),
        []
    );
    assert_eq!(
        atlas_dirty_indices(&[first, second], &[first, changed], false, false),
        [1]
    );
    assert_eq!(
        atlas_dirty_indices(&[first, second], &[first, second], true, false),
        [0, 1]
    );
    assert_eq!(
        atlas_dirty_indices(&[], &[first, second], false, true),
        [0, 1]
    );
}

#[test]
fn fixed_atlas_downscales_all_windows_proportionally() {
    let sizes = [vk::Extent2D {
        width: 1920,
        height: 1080,
    }; 3];
    let (extent, rects) = pack_reflection_atlas(&sizes, 1024).unwrap();
    assert_eq!(extent.width, 1024);
    assert_eq!(extent.height, 1024);
    assert_eq!(rects.len(), 3);
    let mut occupied = std::collections::HashSet::new();
    for rect in &rects {
        assert!(rect.extent.width > 1 && rect.extent.width < 1920);
        assert!(rect.extent.height > 1 && rect.extent.height < 1080);
        assert!(
            (rect.extent.width as f64 / rect.extent.height as f64 - 1920.0 / 1080.0).abs() < 0.01
        );
        assert!(rect.extent == rects[0].extent);
        for y in rect.offset.y as u32..rect.offset.y as u32 + rect.extent.height {
            for x in rect.offset.x as u32..rect.offset.x as u32 + rect.extent.width {
                assert!(x < 1024 && y < 1024);
                assert!(occupied.insert((x, y)));
            }
        }
    }
    let (_, minimum) = pack_reflection_atlas(
        &[vk::Extent2D {
            width: 100,
            height: 1,
        }; 16],
        4,
    )
    .unwrap();
    assert_eq!(minimum.len(), 16);
    assert!(
        minimum
            .iter()
            .all(|rect| rect.extent.width == 1 && rect.extent.height == 1)
    );
}

#[test]
fn shaders_compile() {
    for (entry, stage) in [
        ("sky_vertex", naga::ShaderStage::Vertex),
        ("vertex", naga::ShaderStage::Vertex),
        ("window", naga::ShaderStage::Fragment),
        ("cursor", naga::ShaderStage::Fragment),
        ("environment", naga::ShaderStage::Fragment),
    ] {
        for ambient_occlusion in [false, true] {
            assert_eq!(
                shader(entry, stage, false, ambient_occlusion).unwrap()[0],
                0x0723_0203
            );
        }
        for trace_transparent in [false, true] {
            for ambient_occlusion in [false, true] {
                assert_eq!(
                    shader(
                        "environment",
                        naga::ShaderStage::Fragment,
                        trace_transparent,
                        ambient_occlusion,
                    )
                    .unwrap()[0],
                    0x0723_0203
                );
            }
        }
    }
    assert_eq!(
        shader("environment", naga::ShaderStage::Fragment, true, false).unwrap()[0],
        0x0723_0203
    );
}

#[test]
fn skybox_mip_chain_reduces_dimensions_and_preserves_constant_color() {
    let base = (0..4 * 2)
        .flat_map(|_| {
            [
                f16::from_f32(2.0),
                f16::from_f32(1.0),
                f16::from_f32(0.5),
                f16::from_f32(1.0),
            ]
        })
        .collect();
    let (pixels, mips) = build_skybox_mips(4, 2, base);

    assert_eq!(
        mips.iter()
            .map(|mip| (mip.width, mip.height))
            .collect::<Vec<_>>(),
        [(4, 2), (2, 1), (1, 1)]
    );
    assert_eq!(pixels.len(), (4 * 2 + 2 + 1) * 4);
    for mip in mips {
        let offset = mip.buffer_offset as usize / std::mem::size_of::<f16>();
        for pixel in pixels[offset..offset + (mip.width * mip.height * 4) as usize].chunks(4) {
            assert_eq!(pixel[0].to_f32(), 2.0);
            assert_eq!(pixel[1].to_f32(), 1.0);
            assert_eq!(pixel[2].to_f32(), 0.5);
            assert_eq!(pixel[3].to_f32(), 1.0);
        }
    }
}

#[test]
fn skybox_hdr_sanitization_replaces_nan_and_clamps_values() {
    let mut pixels = [
        f32::NAN,
        -2.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        3.0,
        4.0,
        5.0,
        6.0,
    ];
    sanitize_hdr_pixels(&mut pixels, 123).unwrap();

    assert_eq!(pixels[0], 3.0);
    assert_eq!(pixels[1], 0.0);
    assert_eq!(pixels[2], SKYBOX_MAX_CHANNEL);
    assert_eq!(pixels[3], 0.0);
    assert!(pixels.iter().all(|value| value.is_finite()));
}

#[test]
fn skybox_hdr_sanitization_handles_nan_only_images() {
    let mut pixels = [f32::NAN; 4];
    sanitize_hdr_pixels(&mut pixels, 123).unwrap();
    assert_eq!(pixels, [0.0; 4]);
}

#[test]
fn skybox_diffuse_integrates_only_the_upper_hemisphere() {
    let mut pixels = Vec::new();
    for y in 0..4 {
        let color = if y < 2 {
            [2.0, 1.0, 0.5]
        } else {
            [100.0, 100.0, 100.0]
        };
        for _ in 0..4 {
            pixels.extend([color[0], color[1], color[2], 1.0]);
        }
    }

    let rgba = bytemuck::cast_slice::<f32, Vec4>(&pixels);
    assert_eq!(rgba[0].to_array(), [2.0, 1.0, 0.5, 1.0]);
    let diffuse = integrate_skybox_diffuse(4, 4, rgba);
    assert!((diffuse[0] - 2.0).abs() < 1.0e-6, "{diffuse:?}");
    assert!((diffuse[1] - 1.0).abs() < 1.0e-6, "{diffuse:?}");
    assert!((diffuse[2] - 0.5).abs() < 1.0e-6, "{diffuse:?}");
}

#[test]
fn skybox_diffuse_uses_lambertian_cosine_weighting() {
    let mut pixels = vec![0.0; 8 * 4 * 4];
    pixels[0] = 1.0;

    let diffuse = integrate_skybox_diffuse(4, 8, bytemuck::cast_slice::<f32, Vec4>(&pixels));
    let expected = (std::f32::consts::PI / 8.0).sin().powi(2) / 4.0;
    assert!((diffuse[0] - expected).abs() < 1.0e-6);
    assert_eq!(diffuse[1], 0.0);
    assert_eq!(diffuse[2], 0.0);
}

#[test]
fn asymmetric_projection_matches_vulkan_coordinates() {
    let view = xr::View {
        pose: xr::Posef::IDENTITY,
        fov: xr::Fovf {
            angle_left: -0.7,
            angle_right: 0.9,
            angle_up: 0.8,
            angle_down: -0.6,
        },
    };
    let projection = view_projection(&view);
    for (point, expected) in [
        (
            Vec3::new(view.fov.angle_left.tan(), 0.0, -1.0),
            (-1.0, None),
        ),
        (
            Vec3::new(view.fov.angle_right.tan(), 0.0, -1.0),
            (1.0, None),
        ),
        (
            Vec3::new(0.0, view.fov.angle_up.tan(), -1.0),
            (0.0, Some(-1.0)),
        ),
        (
            Vec3::new(0.0, view.fov.angle_down.tan(), -1.0),
            (0.0, Some(1.0)),
        ),
    ] {
        let ndc = projection.project_point3(point);
        if let Some(vertical) = expected.1 {
            assert!((ndc.y - vertical).abs() < 1.0e-5);
        } else {
            assert!((ndc.x - expected.0).abs() < 1.0e-5);
        }
    }
    assert!(
        projection
            .project_point3(Vec3::new(0.0, 0.0, -0.05))
            .z
            .abs()
            < 1.0e-5
    );
    assert!((projection.project_point3(Vec3::new(0.0, 0.0, -100.0)).z - 1.0).abs() < 1.0e-5);
}

#[test]
fn eye_pose_and_panel_transform_preserve_top_left_coordinates() {
    let orientation = Quat::from_rotation_y(0.4) * Quat::from_rotation_x(-0.2);
    let position = Vec3::new(0.3, 1.7, 0.1);
    let view = xr::View {
        pose: xr::Posef {
            orientation: xr::Quaternionf {
                x: orientation.x,
                y: orientation.y,
                z: orientation.z,
                w: orientation.w,
            },
            position: xr::Vector3f {
                x: position.x,
                y: position.y,
                z: position.z,
            },
        },
        fov: xr::Fovf {
            angle_left: -std::f32::consts::FRAC_PI_4,
            angle_right: std::f32::consts::FRAC_PI_4,
            angle_up: std::f32::consts::FRAC_PI_4,
            angle_down: -std::f32::consts::FRAC_PI_4,
        },
    };
    let geometry = PanelGeometry {
        pose: PanelPose {
            center: position + orientation * Vec3::NEG_Z * 2.0,
            yaw: 0.4,
            pitch: -0.2,
            width_m: 2.0,
        },
        logical_size: (200, 100).into(),
    };
    let transform = view_projection(&view) * model(geometry);
    let center = transform.project_point3(Vec3::ZERO);
    assert!(center.x.abs() < 1.0e-5 && center.y.abs() < 1.0e-5);
    let top_left = transform.project_point3(Vec3::new(-0.5, 0.5, 0.0));
    assert!((top_left.x + 0.5).abs() < 1.0e-5);
    assert!((top_left.y + 0.25).abs() < 1.0e-5);
}

#[test]
fn expanded_border_grows_equally_around_panel_center() {
    let geometry = PanelGeometry {
        pose: PanelPose {
            center: Vec3::new(0.2, 0.3, -2.0),
            width_m: 2.0,
            ..PanelPose::for_slot(0)
        },
        logical_size: (200, 100).into(),
    };
    let expanded = expanded_window_model(geometry, 10.0, 4.0);
    let center = expanded.transform_point3(Vec3::ZERO);
    let left = expanded.transform_point3(Vec3::new(-0.5, 0.0, 0.0));
    let right = expanded.transform_point3(Vec3::new(0.5, 0.0, 0.0));
    assert!((center - geometry.pose.center).length() < 1.0e-5);
    assert!((geometry.pose.center.x - left.x - (right.x - geometry.pose.center.x)).abs() < 1.0e-5);
    assert!((right.x - left.x - 2.26).abs() < 1.0e-5);
}
