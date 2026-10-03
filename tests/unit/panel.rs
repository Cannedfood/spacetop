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
