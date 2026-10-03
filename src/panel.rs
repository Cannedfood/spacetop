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
    #[cfg(test)]
    pub fn for_slot(slot: usize) -> Self {
        Self::for_slot_at_distance(slot, crate::config::DEFAULT_DISTANCE)
    }

    pub fn for_slot_at_distance(slot: usize, distance: f32) -> Self {
        let column = slot.div_ceil(2) as f32 * if slot.is_multiple_of(2) { -1.0 } else { 1.0 };
        Self::facing_origin(Vec3::new(column * 1.1, 0.0, -distance))
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
    pub fn from_bounds(
        root_pose: PanelPose,
        root_size: Size<i32, smithay::utils::Logical>,
        bounds: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) -> Self {
        let meters_per_pixel = root_pose.width_m / root_size.w as f32;
        let offset = Vec3::new(
            bounds.loc.x as f32 + (bounds.size.w - root_size.w) as f32 * 0.5,
            -bounds.loc.y as f32 - (bounds.size.h - root_size.h) as f32 * 0.5,
            0.0,
        ) * meters_per_pixel;
        Self {
            pose: PanelPose {
                center: root_pose.center + root_pose.orientation() * offset,
                width_m: meters_per_pixel * bounds.size.w as f32,
                ..root_pose
            },
            logical_size: bounds.size,
        }
    }

    pub fn root_pose(
        self,
        root_size: Size<i32, smithay::utils::Logical>,
        bounds: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) -> PanelPose {
        let meters_per_pixel = self.pose.width_m / bounds.size.w as f32;
        let offset = Vec3::new(
            bounds.loc.x as f32 + (bounds.size.w - root_size.w) as f32 * 0.5,
            -bounds.loc.y as f32 - (bounds.size.h - root_size.h) as f32 * 0.5,
            0.0,
        ) * meters_per_pixel;
        PanelPose {
            center: self.pose.center - self.pose.orientation() * offset,
            width_m: meters_per_pixel * root_size.w as f32,
            ..self.pose
        }
    }

    /// Intersect a ray with this panel. Returns `None` for parallel, behind-ray,
    /// or out-of-bounds intersections.
    pub fn intersect(&self, ray: Ray3) -> Option<PanelHit> {
        self.intersect_with_margin(ray, 0.0)
    }

    /// Intersect the panel and its resize grab margin, measured as a fraction of its size.
    pub fn intersect_with_margin(&self, ray: Ray3, margin: f32) -> Option<PanelHit> {
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
        if u < -margin || u > 1.0 + margin || v < -margin || v > 1.0 + margin {
            return None;
        }

        Some(PanelHit {
            uv: [u, v],
            surface_px: Vec2::new(
                u.clamp(0.0, 1.0) * pixel_width as f32,
                v.clamp(0.0, 1.0) * pixel_height as f32,
            ),
            distance_m: distance,
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/panel.rs"]
mod tests;
