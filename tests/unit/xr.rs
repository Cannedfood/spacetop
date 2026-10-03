use super::*;
use crate::scene::FALLBACK_FLOOR_Y;

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
        pose: PanelPose::facing_origin(glam::Vec3::new(0.0, 0.0, -2.5)),
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
    let mut location = xr::SpaceLocation {
        location_flags: xr::SpaceLocationFlags::EMPTY,
        pose: xr::Posef::IDENTITY,
    };
    location.pose.position.y = -1.7;
    assert_eq!(
        tracked_floor_height(FALLBACK_FLOOR_Y, location),
        FALLBACK_FLOOR_Y
    );
    location.location_flags = xr::SpaceLocationFlags::POSITION_VALID;
    assert_eq!(tracked_floor_height(FALLBACK_FLOOR_Y, location), -1.7);
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
