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
        let resized = geometry.resized_pose_from_edges(
            (geometry.logical_size.w + 1, geometry.logical_size.h).into(),
            [false, true, false, false],
            32.0,
        );
        assert!(resized.width_m > width_m);
        assert!(resized.width_m < width_m * 1.01);
    }
}

#[test]
fn pixel_grab_margin_accepts_hits_just_outside_panel_bounds() {
    let geometry = panel();
    let ray = Ray3 {
        origin: Vec3::new(-0.505, 0.0, -2.0),
        direction: Vec3::NEG_Z,
    };

    assert!(geometry.intersect(ray).is_none());
    let hit = geometry.intersect_with_margin_px(ray, 6.0).unwrap();
    assert!((hit.surface_px.x + 5.0).abs() < 0.001);
    assert!(geometry.intersect_with_margin_px(ray, 4.0).is_none());
}

#[test]
fn resize_handles_are_outside_the_panel_not_inside_its_edges() {
    let geometry = panel();
    let inside_edge_ray = Ray3 {
        origin: Vec3::new(-0.499, 0.0, -2.0),
        direction: Vec3::NEG_Z,
    };
    let inside_hit = geometry.intersect_unbounded(inside_edge_ray).unwrap();
    assert_eq!(geometry.resize_edges_from_hit(inside_hit), [false; 4]);

    let outside_edge_ray = Ray3 {
        origin: Vec3::new(-0.505, 0.0, -2.0),
        direction: Vec3::NEG_Z,
    };
    let outside_hit = geometry
        .intersect_with_margin_px(outside_edge_ray, 6.0)
        .unwrap();
    assert_eq!(
        geometry.resize_edges_from_hit(outside_hit),
        [true, false, false, false]
    );
}

#[test]
fn pixel_intersection_margins_are_uniform_across_rectangular_axes() {
    let geometry = panel();
    for origin in [Vec3::new(-0.505, 0.0, -2.0), Vec3::new(0.0, 0.2525, -2.0)] {
        let ray = Ray3 {
            origin,
            direction: Vec3::NEG_Z,
        };
        let hit = geometry.intersect_with_margin_px(ray, 6.0).unwrap();
        assert!(hit.surface_px.x >= -6.0 && hit.surface_px.y >= -6.0);
        assert!(geometry.intersect_with_margin_px(ray, 2.0).is_none());
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
fn capture_uses_native_size_hidpi_and_aspect_preserving_limits() {
    let limits = PanelLimits {
        max_width: 1600,
        max_height: 1000,
        max_layers: 4,
    };
    for (logical, buffer_scale, expected) in [
        ((800, 400), 1, (800, 400)),
        ((800, 400), 2, (1600, 800)),
        ((3200, 1600), 1, (1600, 800)),
        ((600, 3000), 2, (200, 1000)),
    ] {
        let (size, _) = limits.capture_size(logical.into(), buffer_scale);
        assert_eq!((size.w, size.h), expected);
    }
}

#[test]
fn placement_slots_are_unique_and_nonoverlapping() {
    assert_eq!(PanelPose::for_slot(0).center, Vec3::new(0.0, 0.0, -1.6));
    for first in 0..32 {
        for second in first + 1..32 {
            let first_pose = PanelPose::for_slot(first);
            let second_pose = PanelPose::for_slot(second);
            assert!((first_pose.center.x - second_pose.center.x).abs() > first_pose.width_m);
        }
    }
}

#[test]
fn dodge_windows_moves_overlapping_panels_and_keeps_fixed_pose() {
    let first = panel();
    let second = PanelGeometry {
        pose: first.pose,
        ..first
    };
    let moved = dodge_windows(&[(1, first), (2, second)], &[1], Vec3::ZERO, 0.05);

    assert_eq!(moved[&1], first.pose);
    assert_ne!(moved[&2].center, second.pose.center);
    assert!((moved[&2].center.length() - second.pose.center.length()).abs() < 1.0e-5);
    let original_angles = PanelPose::spherical_angles(second.pose.center);
    let moved_angles = PanelPose::spherical_angles(moved[&2].center);
    assert!((moved_angles.y - original_angles.y).abs() < 1.0e-5);
}

#[test]
fn dodge_windows_separates_multiple_panels_with_margin() {
    let first = panel();
    let panels = [(1, first), (2, first), (3, first)];
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
    let pose = PanelPose::facing_player(center, player);
    assert!((pose.center.distance(player) - center.distance(player)).abs() < 1.0e-5);
    let normal = pose.orientation() * Vec3::Z;
    assert!(normal.dot((player - pose.center).normalize()) > 0.99999);

    let slot_pose = PanelPose::for_slot(1);
    assert!((slot_pose.orientation() * Vec3::Z).dot(-slot_pose.center.normalize()) > 0.99999);
}

#[test]
fn center_stays_put_when_grab_starts_and_tracks_aim_on_sphere() {
    let initial = PanelPose::facing_origin(Vec3::new(0.7, 0.3, -1.8));
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
fn panel_width_scales_linearly_with_distance() {
    let start_width = 1.0;
    let start_distance = 1.5;
    assert_eq!(
        PanelPose::width_for_distance(start_width, start_distance, start_distance),
        start_width
    );
    assert!((PanelPose::width_for_distance(start_width, start_distance, 3.0) - 2.0).abs() < 1.0e-6);
    assert!((PanelPose::width_for_distance(0.8, 2.0, 1.0) - 0.4).abs() < 1.0e-6);
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
fn edge_resize_keeps_the_opposite_edge_anchored() {
    let initial = panel();
    let resized_width = initial.pose.width_m;
    let unchanged =
        initial.resized_pose_from_edges(initial.logical_size, [false, true, false, false], 32.0);
    assert_eq!(unchanged, initial.pose);

    let right_resize =
        initial.resized_pose_from_edges((1200, 500).into(), [false, true, false, false], 32.0);
    let right_axis = initial.pose.orientation() * Vec3::X;
    let initial_left = initial.pose.center.dot(right_axis) - initial.pose.width_m * 0.5;
    let resized_left = right_resize.center.dot(right_axis) - right_resize.width_m * 0.5;
    assert!((resized_left - initial_left).abs() < 1.0e-5);

    let left_resize =
        initial.resized_pose_from_edges((800, 500).into(), [true, false, false, false], 32.0);
    let initial_right = initial.pose.center.dot(right_axis) + initial.pose.width_m * 0.5;
    let resized_right = left_resize.center.dot(right_axis) + left_resize.width_m * 0.5;
    assert!((resized_right - initial_right).abs() < 1.0e-5);

    let bottom_resize =
        initial.resized_pose_from_edges((1000, 700).into(), [false, false, false, true], 32.0);
    let up_axis = initial.pose.orientation() * Vec3::Y;
    let initial_top = initial.pose.center.dot(up_axis) + resized_width * 0.25;
    let resized_top = bottom_resize.center.dot(up_axis) + bottom_resize.width_m * 0.35;
    assert!((resized_top - initial_top).abs() < 1.0e-5);

    let top_resize =
        initial.resized_pose_from_edges((1000, 300).into(), [false, false, true, false], 32.0);
    let initial_bottom = initial.pose.center.dot(up_axis) - resized_width * 0.25;
    let resized_bottom = top_resize.center.dot(up_axis) - top_resize.width_m * 0.15;
    assert!((resized_bottom - initial_bottom).abs() < 1.0e-5);
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
        .intersect_unbounded(Ray3 {
            origin: Vec3::ZERO,
            direction: Vec3::new(0.6, 0.0, -2.0).normalize(),
        })
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
