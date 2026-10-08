use super::*;

fn panel() -> PanelGeometry {
    PanelGeometry {
        pose: PanelPose {
            center: Vec3::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            pitch: 0.0,
            width_m: 1.0,
        },
        logical_size: Size::from((1000, 500)),
    }
}

#[test]
fn panel_history_uses_layout_precedence_for_all_state_flags() {
    use crate::bridge::PanelState;

    let mut history = PanelHistory::default();
    let regular = PanelPastState {
        distance: 1.0,
        size: (100, 50).into(),
    };
    let maximized = PanelPastState {
        distance: 2.0,
        size: (200, 100).into(),
    };
    let fullscreen = PanelPastState {
        distance: 3.0,
        size: (300, 150).into(),
    };
    history.save(PanelState::default(), regular);
    history.save(
        PanelState {
            maximized: true,
            ..Default::default()
        },
        maximized,
    );
    history.save(
        PanelState {
            fullscreen: true,
            ..Default::default()
        },
        fullscreen,
    );

    for minimized in [false, true] {
        for maximized_flag in [false, true] {
            for fullscreen_flag in [false, true] {
                let state = PanelState {
                    minimized,
                    maximized: maximized_flag,
                    fullscreen: fullscreen_flag,
                };
                let expected = if fullscreen_flag {
                    fullscreen
                } else if maximized_flag {
                    maximized
                } else {
                    regular
                };
                assert_eq!(history.get(state), Some(expected));
                let replacement = PanelPastState {
                    distance: 4.0,
                    ..expected
                };
                let mut updated = history;
                updated.save(state, replacement);
                assert_eq!(updated.get(state), Some(replacement));
                assert_eq!(history.get(state), Some(expected));
            }
        }
    }
}

#[test]
fn edge_resize_preserves_the_existing_scale_for_small_size_changes() {
    let geometry = panel();
    for width_m in [0.5, 1.0, 2.0] {
        let geometry = PanelGeometry {
            pose: PanelPose {
                width_m,
                ..geometry.pose
            },
            ..geometry
        };
        let resized = geometry.resized_pose(
            (geometry.logical_size.w + 1, geometry.logical_size.h).into(),
            geometry.pixels_per_degree(),
        );
        assert!(resized.width_m > width_m);
        assert!(resized.width_m < width_m * 1.01);
    }
}

#[test]
fn maximized_surface_size_uses_angular_bounds_and_logical_pixel_density() {
    let size = panel().size_for_angular_bounds(2.0, 32.0, 70.0, 50.0);
    assert_eq!(size, (2240, 1492));
}

#[test]
fn negotiated_size_fills_both_angular_bounds_independently_of_original_aspect() {
    for original_size in [(1000, 500), (300, 1200)] {
        let geometry = PanelGeometry {
            logical_size: original_size.into(),
            ..panel()
        };
        for (width, height) in [(70.0, 50.0), (100.0, 65.0)] {
            let size = geometry.size_for_angular_bounds(2.0, 32.0, width, height);
            let fitted = PanelGeometry::fit_pose_to_angular_bounds(
                geometry.pose,
                size.into(),
                2.0,
                width,
                height,
            );
            let actual_width = 2.0 * (fitted.width_m / 4.0).atan().to_degrees();
            let actual_height = 2.0
                * (fitted.width_m * size.1 as f32 / size.0 as f32 / 4.0)
                    .atan()
                    .to_degrees();
            assert!((actual_width - width).abs() < 0.03);
            assert!((actual_height - height).abs() < 0.03);
        }
    }
}

#[test]
fn trace_classifies_content_and_handles_with_one_reach_value() {
    let geometry = panel();
    let ray = Ray3 {
        origin: Vec3::ZERO,
        direction: Vec3::NEG_Z * 2.0,
    };
    let center = geometry.trace(ray, 6.0).unwrap();
    assert!(center.on_panel && center.on_content);
    assert_eq!(center.on_edges, [false; 4]);
    assert_eq!(center.position, geometry.pose.center);
    assert_eq!(center.distance_m, 2.0);
    let ray = Ray3 {
        origin: Vec3::new(-0.505, 0.0, 0.0),
        ..ray
    };
    let edge = geometry.trace(ray, 6.0).unwrap();
    assert!(edge.on_panel && !edge.on_content);
    assert_eq!(edge.on_edges, [true, false, false, false]);
    assert!(!geometry.trace(ray, 4.0).unwrap().on_panel);
}

#[test]
fn grab_reach_uses_largest_border_or_padding_plus_margin() {
    let mut window = crate::config::AppConfig::default().window;
    window.padding_px = 100.0;
    window.margin_px = 0.0;
    window.border_width_px = 10.0;
    window.cursor_close_border_width_px = 20.0;
    window.grabbed_border_width_px = 60.0;
    assert_eq!(window.grab_reach_px(), 100.0);
    window.padding_px = 2.0;
    window.margin_px = 3.0;
    assert_eq!(window.grab_reach_px(), 60.0);
}

#[test]
fn pixel_grab_margin_accepts_hits_just_outside_panel_bounds() {
    let geometry = panel();
    let ray = Ray3 {
        origin: Vec3::new(-0.505, 0.0, -2.0),
        direction: Vec3::NEG_Z,
    };

    assert!(geometry.intersect(ray).is_none());
    let hit = geometry.trace(ray, 6.0).filter(|hit| hit.on_panel).unwrap();
    assert!((hit.surface_px.x + 5.0).abs() < 0.001);
    assert!(!geometry.trace(ray, 4.0).unwrap().on_panel);
}

#[test]
fn resize_handles_are_outside_the_panel_not_inside_its_edges() {
    let geometry = panel();
    let inside_edge_ray = Ray3 {
        origin: Vec3::new(-0.499, 0.0, -2.0),
        direction: Vec3::NEG_Z,
    };
    let inside_hit = geometry.trace(inside_edge_ray, 0.0).unwrap();
    assert_eq!(inside_hit.on_edges, [false; 4]);

    let outside_edge_ray = Ray3 {
        origin: Vec3::new(-0.505, 0.0, -2.0),
        direction: Vec3::NEG_Z,
    };
    let outside_hit = geometry.trace(outside_edge_ray, 6.0).unwrap();
    assert_eq!(outside_hit.on_edges, [true, false, false, false]);
}

#[test]
fn pixel_intersection_margins_are_uniform_across_rectangular_axes() {
    let geometry = panel();
    for origin in [Vec3::new(-0.505, 0.0, -2.0), Vec3::new(0.0, 0.2525, -2.0)] {
        let ray = Ray3 {
            origin,
            direction: Vec3::NEG_Z,
        };
        let hit = geometry.trace(ray, 6.0).filter(|hit| hit.on_panel).unwrap();
        assert!(hit.surface_px.x >= -6.0 && hit.surface_px.y >= -6.0);
        assert!(!geometry.trace(ray, 2.0).unwrap().on_panel);
    }
}

#[test]
fn expanded_bounds_preserve_root_placement_and_pixel_scale() {
    let root = panel();
    let bounds = smithay::utils::Rectangle::new((-200, -100).into(), (1500, 800).into());
    let expanded = PanelGeometry::from_bounds(root.pose, root.logical_size, bounds);
    let restored = expanded.root_pose(root.logical_size, bounds);
    assert!((restored.center - root.pose.center).length() < 1.0e-6);
    assert!((restored.width_m - root.pose.width_m).abs() < 1.0e-6);
    let ray = Ray3 {
        origin: Vec3::ZERO,
        direction: Vec3::NEG_Z,
    };
    let hit = expanded.intersect(ray).unwrap();
    assert!((hit.surface_px.x + bounds.loc.x as f32 - 500.0).abs() < 0.001);
    assert!((hit.surface_px.y + bounds.loc.y as f32 - 250.0).abs() < 0.001);
}

#[test]
fn capture_uses_display_scale_and_aspect_preserving_limits() {
    let max_panel_size = 1600;
    for (logical, buffer_scale, display_scale, expected) in [
        ((800, 400), 1, 1.0, (800, 400)),
        ((800, 400), 2, 1.0, (1600, 800)),
        ((800, 400), 1, 0.5, (400, 200)),
        ((800, 400), 1, 1.5, (1200, 600)),
        ((800, 400), 2, 1.5, (1600, 800)),
        ((3200, 1600), 1, 1.0, (1600, 800)),
        ((600, 3000), 2, 1.0, (320, 1600)),
    ] {
        let (size, _) = capture_size(max_panel_size, logical.into(), buffer_scale, display_scale);
        assert_eq!((size.w, size.h), expected);
    }
}

#[test]
fn placement_slots_are_unique_and_nonoverlapping() {
    let defaults = crate::config::AppConfig::default().window;
    let first_pose = PanelPose::for_slot(0);
    let first_angles = PanelPose::spherical_angles(first_pose.center);
    assert!((first_pose.center.length() - defaults.default_distance_m).abs() < 1.0e-6);
    assert!((first_angles.y - defaults.default_vertical_angle_degrees.to_radians()).abs() < 1.0e-6);
    for angle in [-90.0, -45.0, 0.0, 45.0, 90.0] {
        for slot in 0..4 {
            let pose = PanelPose::for_slot_at_distance(slot, 2.0, angle);
            let actual_angle = PanelPose::spherical_angles(pose.center).y.to_degrees();
            assert!((actual_angle - angle).abs() < 1.0e-4);
            assert!(pose.center.is_finite());
        }
    }
    for first in 0..32 {
        for second in first + 1..32 {
            let first_pose = PanelPose::for_slot(first);
            let second_pose = PanelPose::for_slot(second);
            assert!((first_pose.center.x - second_pose.center.x).abs() > first_pose.width_m);
        }
    }
}

#[test]
fn new_window_vertical_angle_offsets_the_aim_direction() {
    let aim_pitch = 10.0_f32.to_radians();
    let aim = Vec3::new(0.0, aim_pitch.sin(), -aim_pitch.cos());
    for (offset, expected) in [
        (-90.0_f32, -80.0),
        (-45.0, -35.0),
        (0.0, 10.0),
        (45.0, 55.0),
        (90.0, 90.0),
    ] {
        let pose = PanelPose::on_sphere_from_aim(
            aim,
            Vec2::new(0.0, offset.to_radians()),
            2.0,
            Vec3::ZERO,
        );
        let actual = PanelPose::spherical_angles(pose.center).y.to_degrees();
        assert!((actual - expected).abs() < 1.0e-4);
    }
}

#[test]
fn fullscreen_panel_fits_within_both_angular_bounds() {
    let geometry = panel();
    let distance = 1.6;
    let fitted = PanelGeometry::fit_pose_to_angular_bounds(
        geometry.pose,
        geometry.logical_size,
        distance,
        100.0,
        50.0,
    );
    let width_degrees = 2.0 * (fitted.width_m / (2.0 * distance)).atan().to_degrees();
    let height_m = fitted.width_m * geometry.logical_size.h as f32 / geometry.logical_size.w as f32;
    let height_degrees = 2.0 * (height_m / (2.0 * distance)).atan().to_degrees();

    assert!(width_degrees <= 100.0);
    assert!(height_degrees <= 50.0);
    assert!((height_degrees - 50.0).abs() < 1.0e-4);
    assert_eq!(fitted.center, geometry.pose.center);
    assert_eq!(fitted.yaw, geometry.pose.yaw);
    assert_eq!(fitted.pitch, geometry.pose.pitch);
}

#[test]
fn dodge_windows_moves_overlapping_panels_and_keeps_fixed_pose() {
    let first = panel();
    let second = PanelGeometry {
        pose: first.pose,
        ..first
    };
    let moved = dodge_windows(
        &[(PanelId::new(1), first), (PanelId::new(2), second)],
        &[PanelId::new(1)],
        Vec3::ZERO,
        0.05,
    );

    assert_eq!(moved[&PanelId::new(1)], first.pose);
    assert_ne!(moved[&PanelId::new(2)].center, second.pose.center);
    assert!((moved[&PanelId::new(2)].center.length() - second.pose.center.length()).abs() < 1.0e-5);
    let original_angles = PanelPose::spherical_angles(second.pose.center);
    let moved_angles = PanelPose::spherical_angles(moved[&PanelId::new(2)].center);
    assert!((moved_angles.y - original_angles.y).abs() < 1.0e-5);
}

#[test]
fn dodge_windows_keeps_maximized_grabbed_panel_fixed_while_moving_neighbor() {
    let regular = panel();
    let maximized = PanelGeometry {
        pose: PanelPose {
            width_m: regular.pose.width_m * 2.0,
            ..regular.pose
        },
        ..regular
    };

    let moved = dodge_windows(
        &[(PanelId::new(1), maximized), (PanelId::new(2), regular)],
        &[PanelId::new(1)],
        Vec3::ZERO,
        0.05,
    );

    assert_eq!(moved[&PanelId::new(1)], maximized.pose);
    assert_ne!(moved[&PanelId::new(2)].center, regular.pose.center);
    assert!(
        (moved[&PanelId::new(2)].center.length() - regular.pose.center.length()).abs() < 1.0e-5
    );
}

#[test]
fn dodge_windows_separates_multiple_panels_with_margin() {
    let first = panel();
    let panels = [
        (PanelId::new(1), first),
        (PanelId::new(2), first),
        (PanelId::new(3), first),
    ];
    let moved = dodge_windows(&panels, &[], Vec3::ZERO, 0.05);
    let geometries: Vec<_> = panels
        .iter()
        .map(|(id, geometry)| PanelGeometry {
            pose: moved[id],
            ..*geometry
        })
        .collect();

    for first_index in 0..geometries.len() {
        for second_index in first_index + 1..geometries.len() {
            let first_angles = PanelPose::spherical_angles(geometries[first_index].pose.center);
            let second_angles = PanelPose::spherical_angles(geometries[second_index].pose.center);
            let yaw_distance = PanelPose::wrap_angle(first_angles.x - second_angles.x).abs() * 2.0;
            assert!(yaw_distance > 0.45);
        }
    }
    for (id, geometry) in &panels {
        let original_pitch = PanelPose::spherical_angles(geometry.pose.center).y;
        let moved_pitch = PanelPose::spherical_angles(moved[id].center).y;
        assert!((moved_pitch - original_pitch).abs() < 1.0e-5);
    }
}

#[test]
fn grabbed_panel_stays_on_player_sphere_and_faces_player() {
    let player = Vec3::new(0.0, 1.6, 0.0);
    let center = Vec3::new(0.2, 0.4, -2.0);
    let pose = PanelPose::looking_from_to(center, player);
    assert!((pose.center.distance(player) - center.distance(player)).abs() < 1.0e-5);
    let normal = pose.orientation() * Vec3::Z;
    assert!(normal.dot((player - pose.center).normalize()) > 0.99999);

    let slot_pose = PanelPose::for_slot(1);
    assert!((slot_pose.orientation() * Vec3::Z).dot(-slot_pose.center.normalize()) > 0.99999);
}

#[test]
fn center_stays_put_when_grab_starts_and_tracks_aim_on_sphere() {
    let initial = PanelPose::looking_from_to(Vec3::new(0.7, 0.3, -1.8), Vec3::ZERO);
    let aim_direction = Vec3::new(0.2, 0.1, -1.0).normalize();
    let aim_angles = PanelPose::spherical_angles(aim_direction);
    let center_angles = PanelPose::spherical_angles(initial.center);
    let angular_offset = Vec2::new(
        PanelPose::wrap_angle(center_angles.x - aim_angles.x),
        center_angles.y - aim_angles.y,
    );
    let moved = PanelPose::on_sphere_from_aim(
        aim_direction,
        angular_offset,
        initial.center.length(),
        Vec3::ZERO,
    );
    assert!((moved.center - initial.center).length() < 1.0e-4);
    assert!((moved.center.length() - initial.center.length()).abs() < 1.0e-5);

    let moved_aim = Vec3::new(-0.4, 0.3, 1.0).normalize();
    let player = Vec3::new(0.0, 1.6, 0.0);
    let moved = PanelPose::on_sphere_from_aim(moved_aim, angular_offset, 2.0, player);
    assert!((moved.center.distance(player) - 2.0).abs() < 1.0e-5);
    let moved_angles = PanelPose::spherical_angles(moved.center - player);
    let expected_angles = PanelPose::spherical_angles(moved_aim) + angular_offset;
    assert!(PanelPose::wrap_angle(moved_angles.x - expected_angles.x).abs() < 1.0e-5);
    assert!((moved_angles.y - expected_angles.y).abs() < 1.0e-5);
    assert!(
        moved
            .orientation()
            .mul_vec3(Vec3::Z)
            .dot((player - moved.center).normalize())
            > 0.99999
    );
}

#[test]
fn pixel_density_sets_angular_width_and_resize_grows_the_panel() {
    let width = PanelPose::width_for_pixel_density(640.0, 2.0, 32.0);
    let resized = PanelPose::width_for_pixel_density(960.0, 2.0, 32.0);
    assert!(resized > width);
    let angle = 2.0 * (width / 4.0).atan().to_degrees();
    assert!((640.0 / angle - 32.0).abs() < 1.0e-4);
}

#[test]
fn centered_resize_doubles_each_axis_delta_and_accumulates_uncommitted_requests() {
    let geometry = panel();
    let previous = Ray3 {
        origin: Vec3::ZERO,
        direction: Vec3::NEG_Z,
    };
    let current = Ray3 {
        origin: Vec3::new(0.01, -0.02, 0.0),
        ..previous
    };
    for (edges, expected) in [
        ([true, false, true, false], Vec2::new(980.0, 460.0)),
        ([false, true, false, true], Vec2::new(1020.0, 540.0)),
        ([false, true, false, false], Vec2::new(1020.0, 500.0)),
        ([false, false, false, true], Vec2::new(1000.0, 540.0)),
    ] {
        let size = geometry
            .resized_size_from_rays(previous, current, Vec2::new(1000.0, 500.0), edges)
            .unwrap();
        assert!((size - expected).length() < 0.001);
        let next = Ray3 {
            origin: current.origin * 2.0,
            ..current
        };
        let accumulated = geometry
            .resized_size_from_rays(current, next, size, edges)
            .unwrap();
        assert!((accumulated - (expected * 2.0 - Vec2::new(1000.0, 500.0))).length() < 0.001);
    }
}

#[test]
fn edge_resize_keeps_the_center_fixed() {
    let initial = panel();
    let unchanged = initial.resized_pose(initial.logical_size, initial.pixels_per_degree());
    assert_eq!(unchanged, initial.pose);

    let right_resize = initial.resized_pose((1200, 500).into(), initial.pixels_per_degree());
    assert_eq!(right_resize.center, initial.pose.center);
    assert!(right_resize.width_m > initial.pose.width_m);

    let left_resize = initial.resized_pose((800, 500).into(), initial.pixels_per_degree());
    assert_eq!(left_resize.center, initial.pose.center);
    assert!(left_resize.width_m < initial.pose.width_m);

    let bottom_resize = initial.resized_pose((1000, 700).into(), initial.pixels_per_degree());
    assert_eq!(bottom_resize, initial.pose);

    let top_resize = initial.resized_pose((1000, 300).into(), initial.pixels_per_degree());
    assert_eq!(top_resize, initial.pose);
}

#[test]
fn center_ray_hits_center_of_panel() {
    let hit = panel()
        .intersect(Ray3 {
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
        })
        .unwrap();
    assert_eq!(hit.uv, [0.5, 0.5]);
    assert_eq!(hit.surface_px, Vec2::new(500.0, 250.0));
    assert!((hit.distance_m - 2.0).abs() < 1.0e-6);
}

#[test]
fn ray_misses_outside_panel_and_behind_origin() {
    assert!(
        panel()
            .intersect(Ray3 {
                origin: Vec3::new(2.0, 0.0, 0.0),
                direction: Vec3::NEG_Z,
            })
            .is_none()
    );
    assert!(
        panel()
            .intersect(Ray3 {
                origin: Vec3::new(0.0, 0.0, -3.0),
                direction: Vec3::NEG_Z,
            })
            .is_none()
    );
}

#[test]
fn unbounded_intersection_preserves_outside_panel_coordinates() {
    let hit = panel()
        .trace(
            Ray3 {
                origin: Vec3::ZERO,
                direction: Vec3::new(0.6, 0.0, -2.0).normalize(),
            },
            0.0,
        )
        .unwrap();
    assert!(hit.surface_px.x > panel().logical_size.w as f32);
}

#[test]
fn rotated_panel_maps_to_top_left_origin_coordinates() {
    let geometry = PanelGeometry {
        pose: PanelPose {
            center: Vec3::new(2.0, 0.0, 0.0),
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
            ..panel().pose
        },
        ..panel()
    };
    let hit = geometry
        .intersect(Ray3 {
            origin: Vec3::new(2.0, 0.0, 0.0),
            direction: Vec3::NEG_X,
        })
        .unwrap();
    assert!((hit.uv[0] - 0.5).abs() < 1.0e-5);
    assert!((hit.uv[1] - 0.5).abs() < 1.0e-5);
}
