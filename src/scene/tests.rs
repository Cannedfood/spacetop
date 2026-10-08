use super::*;

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
