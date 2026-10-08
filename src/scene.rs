use ash::vk;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use openxr as xr;
use std::ops::{Deref, DerefMut};

use crate::panel::{PanelGeometry, PanelPose};

mod atlas;
mod environment;
mod frame;
mod pipeline;
mod renderer;
mod resources;
mod skybox;

use atlas::ReflectionAtlas;
pub(crate) use resources::{PanelTexture, RenderTarget};
pub(crate) use skybox::SkyboxTexture;

pub(crate) fn view_projection(view: &xr::View) -> Mat4 {
    let left = view.fov.angle_left.tan();
    let right = view.fov.angle_right.tan();
    let down = view.fov.angle_down.tan();
    let up = view.fov.angle_up.tan();
    let near = 0.05;
    let far = 100.0;
    let projection = Mat4::from_cols(
        Vec4::new(2.0 / (right - left), 0.0, 0.0, 0.0),
        Vec4::new(0.0, -2.0 / (up - down), 0.0, 0.0),
        Vec4::new(
            (right + left) / (right - left),
            -(up + down) / (up - down),
            far / (near - far),
            -1.0,
        ),
        Vec4::new(0.0, 0.0, far * near / (near - far), 0.0),
    );
    let pose = view.pose;
    let camera = Mat4::from_rotation_translation(
        Quat::from_xyzw(
            pose.orientation.x,
            pose.orientation.y,
            pose.orientation.z,
            pose.orientation.w,
        ),
        Vec3::new(pose.position.x, pose.position.y, pose.position.z),
    );
    projection * camera.inverse()
}

fn model(geometry: PanelGeometry) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::new(
            geometry.pose.width_m,
            geometry.pose.width_m * geometry.logical_size.h as f32 / geometry.logical_size.w as f32,
            1.0,
        ),
        geometry.pose.orientation(),
        geometry.pose.center,
    )
}

fn expanded_window_model(geometry: PanelGeometry, padding_px: f32, border_width_px: f32) -> Mat4 {
    let pixels_per_meter = geometry.logical_size.w as f32 / geometry.pose.width_m;
    let expansion_px = padding_px * 2.0 + border_width_px + 2.0;
    let width_m = (geometry.logical_size.w as f32 + expansion_px) / pixels_per_meter;
    let height_m = (geometry.logical_size.h as f32 + expansion_px) / pixels_per_meter;
    Mat4::from_scale_rotation_translation(
        Vec3::new(width_m, height_m, 1.0),
        geometry.pose.orientation(),
        geometry.pose.center,
    )
}

pub(crate) struct RenderContext {
    device: ash::Device,
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
    render_pass: vk::RenderPass,
    descriptor_layout: vk::DescriptorSetLayout,
    floor_descriptor_layout: vk::DescriptorSetLayout,
    environment_descriptor_layout: vk::DescriptorSetLayout,
    sampler: vk::Sampler,
    sky_sampler: vk::Sampler,
    layout: vk::PipelineLayout,
    window_pipeline: vk::Pipeline,
    cursor_pipeline: vk::Pipeline,
    environment_pipelines: [vk::Pipeline; 4],
}

struct FloorRenderer {
    pool: vk::DescriptorPool,
    descriptor: vk::DescriptorSet,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

struct EnvironmentRenderer {
    pool: vk::DescriptorPool,
    descriptor: vk::DescriptorSet,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    buffer_size: u64,
    descriptor_capacity: u32,
    skybox_view: vk::ImageView,
    diffuse_irradiance: Vec3,
    background_exposure: f32,
    dim: f32,
    max_windows: u32,
}

struct WindowRenderer {
    trace_through_transparent_windows: bool,
    ambient_occlusion: bool,
    window_padding_px: f32,
    max_border_width_px: f32,
}

pub(crate) struct SceneRenderer {
    context: RenderContext,
    floor: FloorRenderer,
    environment: EnvironmentRenderer,
    atlas: ReflectionAtlas,
    window: WindowRenderer,
}

impl Deref for SceneRenderer {
    type Target = RenderContext;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl DerefMut for SceneRenderer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}

pub(crate) struct SceneFrame<'a> {
    pub skybox: Option<&'a SkyboxTexture>,
    pub panels: &'a [(&'a PanelTexture, PanelGeometry)],
    pub cursor: Option<PanelPose>,
    pub cursor_close_panel: Option<(PanelGeometry, Vec2)>,
    pub grabbed_panel: Option<PanelGeometry>,
    pub environment_dim: f32,
    pub floor_y: f32,
    pub texture_sample_phase: u32,
}

#[cfg(test)]
#[path = "scene/tests.rs"]
mod tests;
