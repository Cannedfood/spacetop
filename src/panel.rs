//! Spatial panel geometry, independent of Smithay's 2D desktop abstractions.

use glam::{Vec2, Vec3};
use smithay::utils::Size;

#[derive(Debug, Clone, Copy)]
pub struct PanelLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_layers: u32,
}

impl Default for PanelLimits {
    fn default() -> Self {
        Self {
            max_width: 4096,
            max_height: 4096,
            max_layers: 16,
        }
    }
}

impl PanelLimits {
    pub fn capture_size(
        self,
        logical_size: Size<i32, smithay::utils::Logical>,
        buffer_scale: i32,
    ) -> (Size<i32, smithay::utils::Buffer>, f64) {
        let scale = (self.max_width as f64 / logical_size.w as f64)
            .min(self.max_height as f64 / logical_size.h as f64)
            .min(buffer_scale.max(1) as f64);
        let size = (
            (logical_size.w as f64 * scale).round().max(1.0) as i32,
            (logical_size.h as f64 * scale).round().max(1.0) as i32,
        )
            .into();
        (size, scale)
    }
}

/// A ray in the OpenXR reference space selected by the client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray3 {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// Pose and physical width of a rectangular panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelPose {
    /// Panel center in OpenXR reference-space meters.
    pub center: Vec3,
    /// Rotation about the vertical axis, in radians.
    pub yaw: f32,
    /// Rotation about the local horizontal axis, in radians.
    pub pitch: f32,
    /// Physical panel width in meters.
    pub width_m: f32,
}

impl PanelPose {
    pub fn for_slot(slot: usize) -> Self {
        let column = slot.div_ceil(2) as f32 * if slot.is_multiple_of(2) { -1.0 } else { 1.0 };
        Self::facing_origin(Vec3::new(column * 1.1, 0.0, -1.6))
    }

    /// Place a panel at `center` and orient its front toward the local-space origin.
    pub fn facing_origin(center: Vec3) -> Self {
        Self::facing_player(center, Vec3::ZERO)
    }

    /// Place a panel center relative to a player position, facing back toward that player.
    pub fn facing_player(center: Vec3, player: Vec3) -> Self {
        let toward_player = player - center;
        let radius = toward_player.length().max(f32::EPSILON);
        let yaw = toward_player.x.atan2(toward_player.z);
        let pitch = -(toward_player.y / radius).clamp(-1.0, 1.0).asin();
        Self {
            center,
            yaw,
            pitch,
            width_m: 1.0,
        }
    }

    /// Keep the panel center at a fixed angular offset from the controller aim on the sphere.
    pub fn on_sphere_from_aim(
        aim_direction: Vec3,
        angular_offset: Vec2,
        radius: f32,
        player: Vec3,
    ) -> Self {
        let aim = Self::spherical_angles(aim_direction);
        let yaw = aim.x + angular_offset.x;
        let pitch = (aim.y + angular_offset.y)
            .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
        let direction = Self::direction_from_angles(yaw, pitch);
        Self::facing_player(player + direction * radius, player)
    }

    /// Yaw and pitch in the local reference space, with yaw measured from -Z.
    pub fn spherical_angles(direction: Vec3) -> Vec2 {
        let direction = direction.normalize_or_zero();
        Vec2::new(direction.x.atan2(-direction.z), direction.y.asin())
    }

    pub fn direction_from_angles(yaw: f32, pitch: f32) -> Vec3 {
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    pub fn wrap_angle(angle: f32) -> f32 {
        (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
    }

    /// Scale a panel linearly with viewer distance to preserve its projected size.
    pub fn width_for_distance(base_width: f32, base_distance: f32, distance: f32) -> f32 {
        base_width * distance / base_distance.max(f32::EPSILON)
    }

    pub fn orientation(self) -> glam::Quat {
        glam::Quat::from_rotation_y(self.yaw) * glam::Quat::from_rotation_x(self.pitch)
    }
}

/// A mapped Wayland toplevel's panel placement and current logical size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelGeometry {
    pub pose: PanelPose,
    pub logical_size: Size<i32, smithay::utils::Logical>,
}

/// A hit on a panel, with top-left-origin Wayland surface coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelHit {
    pub uv: [f32; 2],
    pub surface_px: Vec2,
    pub distance_m: f32,
}

impl PanelGeometry {
    /// Intersect a ray with this panel. Returns `None` for parallel, behind-ray,
    /// or out-of-bounds intersections.
    pub fn intersect(&self, ray: Ray3) -> Option<PanelHit> {
        let pixel_width = self.logical_size.w;
        let pixel_height = self.logical_size.h;
        let physical_width = self.pose.width_m;
        if pixel_width <= 0
            || pixel_height <= 0
            || !physical_width.is_finite()
            || physical_width <= 0.0
        {
            return None;
        }

        let aspect = pixel_width as f32 / pixel_height as f32;
        let physical_height = physical_width / aspect;
        let orientation = self.pose.orientation();
        let normal = orientation * Vec3::Z;
        let denominator = ray.direction.dot(normal);
        if denominator.abs() < 1.0e-6 {
            return None;
        }

        let to_center = self.pose.center - ray.origin;
        let distance = to_center.dot(normal) / denominator;
        if !distance.is_finite() || distance < 0.0 {
            return None;
        }

        let world_hit = ray.origin + ray.direction * distance;
        let relative = world_hit - self.pose.center;
        let local = orientation.inverse() * relative;
        let local_x = local.x;
        let local_y = local.y;
        let u = local_x / physical_width + 0.5;
        let v = 0.5 - local_y / physical_height;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }

        Some(PanelHit {
            uv: [u, v],
            surface_px: Vec2::new(u * pixel_width as f32, v * pixel_height as f32),
            distance_m: distance,
        })
    }
}

#[cfg(test)]
mod tests {
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
        assert!(
            (PanelPose::width_for_distance(start_width, start_distance, 3.0) - 2.0).abs() < 1.0e-6
        );
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
}
