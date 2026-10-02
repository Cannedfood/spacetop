//! Spatial panel geometry, independent of Smithay's 2D desktop abstractions.

use glam::{Vec2, Vec3};
use smithay::utils::Size;

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
    /// Physical panel width in meters.
    pub width_m: f32,
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
        let (sin_yaw, cos_yaw) = self.pose.yaw.sin_cos();
        // The panel's front normal is local +Z, rotated into XR space.
        let normal = Vec3::new(sin_yaw, 0.0, cos_yaw);
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
        // Inverse-rotate around Y to obtain panel-local coordinates.
        let local_x = cos_yaw * relative.x - sin_yaw * relative.z;
        let local_y = relative.y;
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
                width_m: 1.0,
            },
            logical_size: Size::from((1000, 500)),
        }
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
