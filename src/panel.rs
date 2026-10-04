//! Spatial panel geometry, independent of Smithay's 2D desktop abstractions.

use glam::{Vec2, Vec3};
use smithay::utils::Size;
use std::collections::{BTreeMap, BTreeSet};

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
        let window = crate::config::AppConfig::default().window;
        Self::for_slot_at_distance(
            slot,
            window.default_distance_m,
            window.default_vertical_angle_degrees,
        )
    }

    pub fn for_slot_at_distance(slot: usize, distance: f32, vertical_angle_degrees: f32) -> Self {
        let column = slot.div_ceil(2) as f32 * if slot.is_multiple_of(2) { -1.0 } else { 1.0 };
        let base_x = column * 1.1;
        let base_distance = base_x.hypot(distance);
        let (sin_angle, cos_angle) = vertical_angle_degrees.to_radians().sin_cos();
        Self::looking_from_to(
            Vec3::new(
                base_x * cos_angle,
                base_distance * sin_angle,
                -distance * cos_angle,
            ),
            Vec3::ZERO,
        )
    }

    /// Place a panel center relative to a player position, facing back toward that player.
    pub fn looking_from_to(center: Vec3, player: Vec3) -> Self {
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
        Self::looking_from_to(player + direction * radius, player)
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

    pub fn width_for_pixel_density(pixel_width: f32, distance: f32, pixels_per_degree: f32) -> f32 {
        let angle = (pixel_width / pixels_per_degree.max(f32::EPSILON))
            .to_radians()
            .clamp(0.0, 170.0_f32.to_radians());
        2.0 * distance.max(f32::EPSILON) * (angle * 0.5).tan()
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

/// Return collision-free target poses, preserving the depth of every panel.
/// Fixed panels anchor the layout; movable panels are placed at the closest
/// available horizontal position to their saved pose.
pub fn dodge_windows(
    panels: &[(u64, PanelGeometry)],
    fixed_windows: &[u64],
    player: Vec3,
    margin_m: f32,
) -> BTreeMap<u64, PanelPose> {
    #[derive(Clone, Copy)]
    struct Bounds {
        center: Vec2,
        half_size: Vec2,
    }

    let fixed: BTreeSet<_> = fixed_windows.iter().copied().collect();
    let Some((_, first)) = panels.first() else {
        return BTreeMap::new();
    };
    let reference_yaw = PanelPose::spherical_angles(first.pose.center - player).x;
    let mut placed = Vec::<Bounds>::new();
    let mut result = BTreeMap::new();
    let mut pending = Vec::new();

    for (id, geometry) in panels {
        let pose = geometry.pose;
        let offset = pose.center - player;
        let distance = offset.length().max(f32::EPSILON);
        let angles = PanelPose::spherical_angles(offset);
        let height_m = pose.width_m * geometry.logical_size.h.max(1) as f32
            / geometry.logical_size.w.max(1) as f32;
        let margin_angle = (margin_m.max(0.0) / distance).atan();
        let bounds = Bounds {
            center: Vec2::new(PanelPose::wrap_angle(angles.x - reference_yaw), angles.y),
            half_size: Vec2::new(
                (pose.width_m * 0.5 / distance).atan() + margin_angle,
                (height_m * 0.5 / distance).atan() + margin_angle,
            ),
        };
        if fixed.contains(id) {
            placed.push(bounds);
            result.insert(*id, pose);
        } else {
            pending.push((*id, pose, distance, bounds));
        }
    }

    for (id, pose, distance, original) in pending {
        let mut x_candidates = vec![original.center.x];
        for other in &placed {
            x_candidates.extend([
                other.center.x - other.half_size.x - original.half_size.x,
                other.center.x + other.half_size.x + original.half_size.x,
            ]);
        }
        if let Some(leftmost) = placed
            .iter()
            .map(|bounds| bounds.center.x - bounds.half_size.x)
            .min_by(f32::total_cmp)
        {
            x_candidates.push(leftmost - original.half_size.x);
        }
        if let Some(rightmost) = placed
            .iter()
            .map(|bounds| bounds.center.x + bounds.half_size.x)
            .max_by(f32::total_cmp)
        {
            x_candidates.push(rightmost + original.half_size.x);
        }
        x_candidates.sort_by(f32::total_cmp);
        x_candidates.dedup_by(|first, second| (*first - *second).abs() < 1.0e-6);

        let target = x_candidates
            .iter()
            .filter(|x| {
                placed.iter().all(|other| {
                    (**x - other.center.x).abs() >= original.half_size.x + other.half_size.x
                        || (original.center.y - other.center.y).abs()
                            >= original.half_size.y + other.half_size.y
                })
            })
            .min_by(|first, second| {
                (*first - original.center.x)
                    .abs()
                    .total_cmp(&(*second - original.center.x).abs())
            })
            .copied()
            .unwrap_or(original.center.x);
        let angles = Vec2::new(reference_yaw + target, original.center.y);
        let target_pose = PanelPose::looking_from_to(
            player + PanelPose::direction_from_angles(angles.x, angles.y) * distance,
            player,
        );
        let target_pose = PanelPose {
            width_m: pose.width_m,
            ..target_pose
        };
        result.insert(id, target_pose);
        placed.push(Bounds {
            center: Vec2::new(target, original.center.y),
            ..original
        });
    }
    result
}

/// A hit on a panel, with top-left-origin Wayland surface coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelHit {
    pub uv: [f32; 2],
    pub surface_px: Vec2,
    pub distance_m: f32,
}

#[derive(Clone, Copy)]
struct PixelMargin {
    horizontal_px: f32,
    vertical_px: f32,
}

impl PanelGeometry {
    pub(crate) fn resize_edges_from_hit(self, hit: PanelHit) -> [bool; 4] {
        let width = self.logical_size.w as f32;
        let height = self.logical_size.h as f32;
        [
            hit.surface_px.x <= 0.0,
            hit.surface_px.x >= width,
            hit.surface_px.y <= 0.0,
            hit.surface_px.y >= height,
        ]
    }

    pub fn resized_pose_from_edges(
        self,
        new_size: Size<i32, smithay::utils::Logical>,
        resize_edges: [bool; 4],
        pixels_per_degree: f32,
    ) -> PanelPose {
        let initial_width = self.logical_size.w.max(1) as f32;
        let initial_height = self.logical_size.h.max(1) as f32;
        let new_width = new_size.w.max(1) as f32;
        let new_height = new_size.h.max(1) as f32;
        if new_size == self.logical_size {
            return self.pose;
        }
        let distance = self.pose.center.length().max(f32::EPSILON);
        let initial_angle_degrees =
            2.0 * (self.pose.width_m / (2.0 * distance)).atan().to_degrees();
        let measured_pixels_per_degree = initial_width / initial_angle_degrees.max(f32::EPSILON);
        let capture_scale = pixels_per_degree.max(f32::EPSILON) / measured_pixels_per_degree;
        let width_m = PanelPose::width_for_pixel_density(
            new_width * capture_scale,
            distance,
            pixels_per_degree,
        );
        let height_m = width_m * new_height / new_width;
        let initial_height_m = self.pose.width_m * initial_height / initial_width;
        let horizontal_direction = if resize_edges[1] {
            1.0
        } else if resize_edges[0] {
            -1.0
        } else {
            0.0
        };
        let vertical_direction = if resize_edges[2] {
            1.0
        } else if resize_edges[3] {
            -1.0
        } else {
            0.0
        };
        let center_offset = Vec3::new(
            horizontal_direction * (width_m - self.pose.width_m) * 0.5,
            vertical_direction * (height_m - initial_height_m) * 0.5,
            0.0,
        );
        PanelPose {
            center: self.pose.center + self.pose.orientation() * center_offset,
            width_m,
            ..self.pose
        }
    }

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
        let margin = margin.max(0.0);
        self.intersect_at(
            ray,
            Some(PixelMargin {
                horizontal_px: self.logical_size.w as f32 * margin,
                vertical_px: self.logical_size.h as f32 * margin,
            }),
        )
    }

    /// Intersect the panel and its grab margin, measured in logical pixels.
    pub fn intersect_with_margin_px(&self, ray: Ray3, margin_px: f32) -> Option<PanelHit> {
        let margin_px = margin_px.max(0.0);
        self.intersect_at(
            ray,
            Some(PixelMargin {
                horizontal_px: margin_px,
                vertical_px: margin_px,
            }),
        )
    }

    pub fn intersect_unbounded(&self, ray: Ray3) -> Option<PanelHit> {
        self.intersect_at(ray, None)
    }

    fn intersect_at(&self, ray: Ray3, margin: Option<PixelMargin>) -> Option<PanelHit> {
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
        let surface_px = Vec2::new(u * pixel_width as f32, v * pixel_height as f32);
        if let Some(margin) = margin
            && (surface_px.x < -margin.horizontal_px
                || surface_px.x > pixel_width as f32 + margin.horizontal_px
                || surface_px.y < -margin.vertical_px
                || surface_px.y > pixel_height as f32 + margin.vertical_px)
        {
            return None;
        }

        Some(PanelHit {
            uv: [u, v],
            surface_px,
            distance_m: distance,
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/panel.rs"]
mod tests;
