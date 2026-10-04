use super::*;

#[test]
fn resize_release_reliably_sends_the_final_size_when_motion_queue_is_full() {
    let (input, receiver) = crate::bridge::input_channel();
    for time_ms in 0..16 {
        input.try_send(XrInput::PointerLost { time_ms }).unwrap();
    }
    let geometry = PanelGeometry {
        pose: PanelPose::looking_from_to(glam::Vec3::new(0.0, 0.0, -1.6), glam::Vec3::ZERO),
        logical_size: (100, 50).into(),
    };
    let edges = [false, true, false, true];
    let mut resizing_panel = Some(7);
    finish_resize(
        &mut PanelImages::new(),
        &input,
        &mut resizing_panel,
        Some(geometry),
        edges,
        Some((140, 90)),
    )
    .unwrap();
    assert!(resizing_panel.is_none());
    for _ in 0..16 {
        let event = receiver.try_recv().unwrap();
        input.received(&event);
        assert!(matches!(event, XrInput::PointerLost { .. }));
    }
    let event = receiver.try_recv().unwrap();
    input.received(&event);
    let XrInput::ResizePanel {
        panel_id,
        width,
        height,
        anchor,
    } = event
    else {
        panic!("resize release must send the final request");
    };
    assert_eq!(panel_id, 7);
    assert_eq!((width, height), (140, 90));
    assert_eq!(anchor, Some((geometry, edges)));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn resize_snapshot_uses_committed_pose_without_animation() {
    let mut current = PanelGeometry {
        pose: PanelPose::looking_from_to(glam::Vec3::new(0.0, 0.0, -1.6), glam::Vec3::ZERO),
        logical_size: (100, 50).into(),
    };
    let mut saved = current.pose;
    let mut temporary = current.pose;
    let committed = PanelGeometry {
        pose: current.resized_pose_from_edges((120, 70).into(), [false, true, false, true], 32.0),
        logical_size: (120, 70).into(),
    };
    reconcile_panel_geometry(&mut current, &mut saved, &mut temporary, committed, true);
    assert_eq!(current, committed);
    assert_eq!(saved, committed.pose);
    assert_eq!(temporary, committed.pose);
}

#[test]
fn cursor_uses_default_player_sphere_without_a_window() {
    let player = glam::Vec3::new(0.4, 1.7, 0.2);
    let ray = Ray3 {
        origin: player + glam::Vec3::new(0.3, -0.2, -0.4),
        direction: glam::Vec3::NEG_Z * 2.0,
    };
    let mut sphere_radius = 1.6;
    let pose = cursor_pose(ray, player, std::iter::empty(), &mut sphere_radius).unwrap();
    assert!((pose.center.distance(player) - 1.6).abs() < 1.0e-5);
    assert!((pose.center - ray.origin).cross(ray.direction).length() < 1.0e-5);
    let normal = pose.orientation() * glam::Vec3::Z;
    assert!(normal.dot((player - pose.center).normalize()) > 0.9999);
}

#[test]
fn cursor_uses_nearest_window_hit_and_orientation() {
    let ray = Ray3 {
        origin: glam::Vec3::new(0.1, 0.0, 0.0),
        direction: glam::Vec3::NEG_Z,
    };
    let near = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -0.8),
            yaw: 0.3,
            pitch: 0.2,
            width_m: 1.0,
        },
        logical_size: (1000, 800).into(),
    };
    let far = PanelGeometry {
        pose: PanelPose::looking_from_to(glam::Vec3::new(0.0, 0.0, -2.5), glam::Vec3::ZERO),
        ..near
    };
    let mut sphere_radius = 1.6;
    let pose = cursor_pose(
        ray,
        glam::Vec3::ZERO,
        [far, near].into_iter(),
        &mut sphere_radius,
    )
    .unwrap();
    let hit = near.intersect(ray).unwrap();
    assert!((pose.center - (ray.origin + ray.direction * hit.distance_m)).length() < 1.0e-5);
    assert_eq!(pose.orientation(), near.pose.orientation());
    assert_eq!(pose.width_m, 0.021);
    assert!((sphere_radius - pose.center.length()).abs() < 1.0e-5);
    let hovered_radius = sphere_radius;
    let miss = Ray3 {
        direction: glam::Vec3::X,
        ..ray
    };
    let pose = cursor_pose(
        miss,
        glam::Vec3::ZERO,
        [far, near].into_iter(),
        &mut sphere_radius,
    )
    .unwrap();
    assert!((pose.center.length() - hovered_radius).abs() < 1.0e-5);
}

#[test]
fn cursor_rejects_invalid_rays_and_sphere_misses() {
    for ray in [
        Ray3 {
            origin: glam::Vec3::ZERO,
            direction: glam::Vec3::ZERO,
        },
        Ray3 {
            origin: glam::Vec3::new(3.0, 0.0, 0.0),
            direction: glam::Vec3::NEG_Z,
        },
        Ray3 {
            origin: glam::Vec3::new(0.0, 0.0, -3.0),
            direction: glam::Vec3::NEG_Z,
        },
    ] {
        let mut sphere_radius = 1.6;
        assert!(
            cursor_pose(
                ray,
                glam::Vec3::ZERO,
                std::iter::empty(),
                &mut sphere_radius
            )
            .is_none()
        );
    }
}

#[test]
fn requires_srgb_swapchain() {
    let srgb = vk::Format::R8G8B8A8_SRGB;
    let linear = vk::Format::R8G8B8A8_UNORM;
    assert_eq!(
        panel_swapchain_format(&[linear.as_raw() as u32, srgb.as_raw() as u32])
            .unwrap()
            .as_raw(),
        srgb.as_raw()
    );
    assert!(panel_swapchain_format(&[linear.as_raw() as u32]).is_err());
    assert!(panel_swapchain_format(&[]).is_err());
}

#[test]
fn stage_floor_uses_valid_height_and_retains_it_during_tracking_loss() {
    let default_floor_height = crate::config::AppConfig::default().floor.height_m;
    let mut location = xr::SpaceLocation {
        location_flags: xr::SpaceLocationFlags::EMPTY,
        pose: xr::Posef::IDENTITY,
    };
    location.pose.position.y = -1.7;
    assert_eq!(
        tracked_floor_height(default_floor_height, location),
        default_floor_height
    );
    location.location_flags = xr::SpaceLocationFlags::POSITION_VALID;
    assert_eq!(tracked_floor_height(default_floor_height, location), -1.7);
    location.pose.position.y = 0.0;
    assert_eq!(tracked_floor_height(-1.7, location), 0.0);
    location.location_flags = xr::SpaceLocationFlags::ORIENTATION_VALID;
    assert_eq!(tracked_floor_height(-1.7, location), -1.7);
    location.location_flags = xr::SpaceLocationFlags::POSITION_VALID;
    for height in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        location.pose.position.y = height;
        assert_eq!(tracked_floor_height(-1.7, location), -1.7);
    }
}
