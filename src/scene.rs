use anyhow::{Context, Result, ensure};
use ash::vk;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4, Vec4Swizzles};
use half::f16;
use openxr as xr;
use rayon::prelude::*;
use smithay::backend::allocator::Buffer;
use std::{
    fs::{self, File},
    io::Read,
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

use crate::{
    config::AppConfig,
    gpu::SharedImage,
    panel::{PanelGeometry, PanelPose},
};

mod frame;
mod pipeline;
mod resources;

pub(crate) use resources::{PanelTexture, RenderTarget, SkyboxTexture};
use resources::{ReflectionAtlasImage, atlas_dirty_indices, pack_reflection_atlas};
#[cfg(test)]
use resources::{
    SKYBOX_MAX_CHANNEL, build_skybox_mips, integrate_skybox_diffuse, sanitize_hdr_pixels,
};

const SHADER_PARTS: [&str; 5] = [
    include_str!("shaders/shared.wgsl"),
    include_str!("shaders/panel.wgsl"),
    include_str!("shaders/tone_mapping.wgsl"),
    include_str!("shaders/lighting.wgsl"),
    include_str!("shaders/environment.wgsl"),
];
static NEXT_PANEL_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PanelTextureId(u64);

fn shader(
    entry: &str,
    stage: naga::ShaderStage,
    trace_through_transparent_windows: bool,
    ambient_occlusion: bool,
) -> Result<Vec<u32>> {
    let mut shader_source = SHADER_PARTS.join("\n").replace(
        "const TRACE_THROUGH_TRANSPARENT_WINDOWS: bool = false;",
        &format!(
            "const TRACE_THROUGH_TRANSPARENT_WINDOWS: bool = {};",
            trace_through_transparent_windows
        ),
    );
    shader_source = shader_source.replace(
        "const AMBIENT_OCCLUSION: bool = false;",
        &format!("const AMBIENT_OCCLUSION: bool = {ambient_occlusion};"),
    );
    let module = naga::front::wgsl::parse_str(&shader_source)
        .map_err(|error| anyhow::anyhow!(error.emit_to_string(&shader_source)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::IMMEDIATES,
    )
    .validate(&module)?;
    let options = naga::back::spv::Options {
        flags: naga::back::spv::WriterFlags::empty(),
        ..Default::default()
    };
    Ok(naga::back::spv::write_vec(
        &module,
        &info,
        &options,
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: stage,
            entry_point: entry.into(),
        }),
    )?)
}

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

fn sky_matrix(view: &xr::View) -> Mat4 {
    let orientation = Quat::from_xyzw(
        view.pose.orientation.x,
        view.pose.orientation.y,
        view.pose.orientation.z,
        view.pose.orientation.w,
    );
    let left = view.fov.angle_left.tan();
    let right = view.fov.angle_right.tan();
    let down = view.fov.angle_down.tan();
    let up = view.fov.angle_up.tan();
    let right_axis = orientation * Vec3::X;
    let up_axis = orientation * Vec3::Y;
    let forward_axis = orientation * Vec3::NEG_Z;
    Mat4::from_cols(
        (right_axis * (right - left)).extend(0.0),
        (up_axis * (down - up)).extend(0.0),
        Vec4::ZERO,
        (right_axis * left + up_axis * up + forward_axis).extend(0.0),
    )
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

fn image_range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(aspect)
        .level_count(1)
        .layer_count(1)
}

fn random_background() -> Result<PathBuf> {
    let directory = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set; cannot locate skybox backgrounds")?
        .join(".config/spacetop/backgrounds");
    let mut backgrounds = fs::read_dir(&directory)
        .with_context(|| format!("read skybox directory {}", directory.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exr"))
        })
        .collect::<Vec<_>>();
    ensure!(
        !backgrounds.is_empty(),
        "no .exr skyboxes found in {}",
        directory.display()
    );
    let mut random = [0; 8];
    File::open("/dev/urandom")
        .context("open system random source")?
        .read_exact(&mut random)
        .context("read system random source")?;
    Ok(backgrounds.swap_remove((u64::from_ne_bytes(random) % backgrounds.len() as u64) as usize))
}

fn background_path(image: &str) -> Result<PathBuf> {
    if image == "random" {
        return random_background();
    }
    if let Some(relative) = image.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set; cannot expand background path")?;
        return Ok(home.join(relative));
    }
    Ok(PathBuf::from(image))
}

fn memory_type(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    bits: u32,
    required: vk::MemoryPropertyFlags,
) -> Result<u32> {
    let properties = unsafe { instance.get_physical_device_memory_properties(physical_device) };
    (0..properties.memory_type_count)
        .find(|index| {
            bits & (1 << index) != 0
                && properties.memory_types[*index as usize]
                    .property_flags
                    .contains(required)
        })
        .context("no compatible Vulkan memory type")
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

struct ReflectionAtlas {
    texture: Option<ReflectionAtlasImage>,
    rects: Vec<vk::Rect2D>,
    panel_ids: Vec<PanelTextureId>,
    dirty_indices: Vec<usize>,
    size: u32,
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

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct FloorUniform {
    albedo: [f32; 4],
    controls: [f32; 4],
    sampling: [f32; 4],
    window_style: [f32; 4],
    border_color: [f32; 4],
    cursor_close_border_color: [f32; 4],
    grabbed_style: [f32; 4],
    grabbed_border_color: [f32; 4],
    diffuse_irradiance: [f32; 4],
    ground_radius: [f32; 4],
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct WindowGpuData {
    center_width: [f32; 4],
    right_height: [f32; 4],
    up: [f32; 4],
    atlas_rect: [f32; 4],
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct WindowBufferHeader {
    count: u32,
    padding: [u32; 3],
}

impl FloorUniform {
    fn from_config(
        config: &AppConfig,
        skybox_diffuse_irradiance: Vec3,
        background_exposure: f32,
    ) -> Self {
        Self {
            albedo: config.floor.albedo,
            controls: [
                config.floor.reflectance,
                config.background.brightness_stops,
                config.floor.roughness,
                config.floor.ray_count as f32,
            ],
            sampling: [
                config.floor.reflection_grain_size_m,
                config.background.rotation_degrees.to_radians(),
                config.window.texture_aa.shader_mode() as f32,
                background_exposure,
            ],
            window_style: [
                config.window.padding_px,
                config.window.border_width_px,
                config.window.border_radius_px,
                config.window.cursor_close_border_width_px,
            ],
            border_color: config.window.border_color,
            cursor_close_border_color: config.window.cursor_close_border_color,
            grabbed_style: [
                config.window.grabbed_border_width_px,
                config.window.cursor_proximity_radius_px,
                0.0,
                0.0,
            ],
            grabbed_border_color: config.window.grabbed_border_color,
            diffuse_irradiance: [
                skybox_diffuse_irradiance.x,
                skybox_diffuse_irradiance.y,
                skybox_diffuse_irradiance.z,
                0.0,
            ],
            ground_radius: [
                config.floor.radius_degrees,
                config.floor.feathering_m,
                0.0,
                0.0,
            ],
        }
    }
}

impl SceneRenderer {
    pub fn new(
        device: &ash::Device,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        format: vk::Format,
        config: &AppConfig,
    ) -> Result<Self> {
        let mut renderer = Self {
            context: RenderContext {
                device: device.clone(),
                instance: instance.clone(),
                physical_device,
                format,
                render_pass: vk::RenderPass::null(),
                descriptor_layout: vk::DescriptorSetLayout::null(),
                floor_descriptor_layout: vk::DescriptorSetLayout::null(),
                environment_descriptor_layout: vk::DescriptorSetLayout::null(),
                sampler: vk::Sampler::null(),
                sky_sampler: vk::Sampler::null(),
                layout: vk::PipelineLayout::null(),
                window_pipeline: vk::Pipeline::null(),
                cursor_pipeline: vk::Pipeline::null(),
                environment_pipelines: [vk::Pipeline::null(); 4],
            },
            floor: FloorRenderer {
                pool: vk::DescriptorPool::null(),
                descriptor: vk::DescriptorSet::null(),
                buffer: vk::Buffer::null(),
                memory: vk::DeviceMemory::null(),
            },
            environment: EnvironmentRenderer {
                pool: vk::DescriptorPool::null(),
                descriptor: vk::DescriptorSet::null(),
                buffer: vk::Buffer::null(),
                memory: vk::DeviceMemory::null(),
                buffer_size: 0,
                descriptor_capacity: 0,
                skybox_view: vk::ImageView::null(),
                diffuse_irradiance: Vec3::ONE,
                background_exposure: 1.0,
                dim: 0.0,
                max_windows: 0,
            },
            atlas: ReflectionAtlas {
                texture: None,
                rects: Vec::new(),
                panel_ids: Vec::new(),
                dirty_indices: Vec::new(),
                size: config.window.reflection_atlas_size.into(),
            },
            window: WindowRenderer {
                trace_through_transparent_windows: config.floor.trace_through_transparent_windows,
                ambient_occlusion: config.floor.ambient_occlusion,
                window_padding_px: config.window.effective_padding_px(),
                max_border_width_px: config.window.max_border_width_px(),
            },
        };
        renderer.validate_atlas_size(renderer.atlas.size)?;
        let attachments = [
            vk::AttachmentDescription::default()
                .format(format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(vk::Format::D32_SFLOAT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
        ];
        let color = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color)
            .depth_stencil_attachment(&depth)];
        let stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
            | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(stages)
            .dst_stage_mask(stages)
            .src_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )];
        unsafe {
            renderer.render_pass = device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses)
                    .dependencies(&dependencies),
                None,
            )?;
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            renderer.descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            renderer.floor_descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&[
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(0)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                ]),
                None,
            )?;
            let limits = instance
                .get_physical_device_properties(physical_device)
                .limits;
            let buffer_capacity = limits
                .max_storage_buffer_range
                .saturating_sub(std::mem::size_of::<WindowBufferHeader>() as u32)
                / std::mem::size_of::<WindowGpuData>() as u32;
            renderer.environment.max_windows = buffer_capacity;
            ensure!(
                renderer.environment.max_windows > 0,
                "Vulkan device has no capacity for reflected window descriptors"
            );
            let environment_bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(3)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(4)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            renderer.environment_descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&environment_bindings),
                None,
            )?;
            let uniform = FloorUniform::from_config(
                config,
                renderer.environment.diffuse_irradiance,
                renderer.environment.background_exposure,
            );
            renderer.floor.buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(std::mem::size_of::<FloorUniform>() as u64)
                    .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                None,
            )?;
            let requirements = device.get_buffer_memory_requirements(renderer.floor.buffer);
            renderer.floor.memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type(
                        instance,
                        physical_device,
                        requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )?),
                None,
            )?;
            device.bind_buffer_memory(renderer.floor.buffer, renderer.floor.memory, 0)?;
            let mapped = device.map_memory(
                renderer.floor.memory,
                0,
                std::mem::size_of::<FloorUniform>() as u64,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(
                (&uniform as *const FloorUniform).cast::<u8>(),
                mapped.cast::<u8>(),
                std::mem::size_of::<FloorUniform>(),
            );
            device.unmap_memory(renderer.floor.memory);
            renderer.floor.pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::UNIFORM_BUFFER,
                        descriptor_count: 1,
                    }]),
                None,
            )?;
            renderer.floor.descriptor = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(renderer.floor.pool)
                    .set_layouts(&[renderer.floor_descriptor_layout]),
            )?[0];
            let buffers = [vk::DescriptorBufferInfo::default()
                .buffer(renderer.floor.buffer)
                .range(std::mem::size_of::<FloorUniform>() as u64)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(renderer.floor.descriptor)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffers)],
                &[],
            );
            renderer.sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            renderer.sky_sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::REPEAT)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )?;
            let constants = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(128)];
            renderer.layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[
                        renderer.descriptor_layout,
                        renderer.floor_descriptor_layout,
                        renderer.environment_descriptor_layout,
                    ])
                    .push_constant_ranges(&constants),
                None,
            )?;
        }
        renderer.window_pipeline = renderer.pipeline("window", false, false)?;
        renderer.cursor_pipeline = renderer.pipeline("cursor", false, false)?;
        renderer.environment_pipelines = [
            renderer.pipeline("environment", false, false)?,
            renderer.pipeline("environment", true, false)?,
            renderer.pipeline("environment", false, true)?,
            renderer.pipeline("environment", true, true)?,
        ];
        Ok(renderer)
    }

    fn validate_atlas_size(&self, size: u32) -> Result<()> {
        let limit = unsafe {
            self.instance
                .get_physical_device_properties(self.physical_device)
        }
        .limits
        .max_image_dimension2_d;
        ensure!(
            size <= limit,
            "reflection atlas size {size} exceeds this GPU's maximum 2D image dimension {limit}"
        );
        let properties = unsafe {
            self.instance.get_physical_device_format_properties(
                self.physical_device,
                vk::Format::R8G8B8A8_SRGB,
            )
        };
        let mut modifiers = vk::DrmFormatModifierPropertiesListEXT::default();
        unsafe {
            self.instance.get_physical_device_format_properties2(
                self.physical_device,
                vk::Format::R8G8B8A8_SRGB,
                &mut vk::FormatProperties2::default().push_next(&mut modifiers),
            );
        }
        let mut entries = vec![
            vk::DrmFormatModifierPropertiesEXT::default();
            modifiers.drm_format_modifier_count as usize
        ];
        modifiers.p_drm_format_modifier_properties = entries.as_mut_ptr();
        unsafe {
            self.instance.get_physical_device_format_properties2(
                self.physical_device,
                vk::Format::R8G8B8A8_SRGB,
                &mut vk::FormatProperties2::default().push_next(&mut modifiers),
            );
        }
        let source_features = entries
            .iter()
            .find(|modifier| modifier.drm_format_modifier == 0)
            .context("GPU lacks linear DRM-modifier support for reflection atlas sources")?
            .drm_format_modifier_tiling_features;
        ensure!(
            properties
                .optimal_tiling_features
                .contains(vk::FormatFeatureFlags::BLIT_DST | vk::FormatFeatureFlags::SAMPLED_IMAGE)
                && source_features.contains(
                    vk::FormatFeatureFlags::BLIT_SRC
                        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
                ),
            "GPU does not support linear-filtered sRGB window blits into the reflection atlas"
        );
        Ok(())
    }

    pub fn update_config(&mut self, config: &AppConfig) -> Result<()> {
        let atlas_size = config.window.reflection_atlas_size.into();
        self.validate_atlas_size(atlas_size)?;
        self.atlas.size = atlas_size;
        self.window.trace_through_transparent_windows =
            config.floor.trace_through_transparent_windows;
        self.window.ambient_occlusion = config.floor.ambient_occlusion;
        self.window.window_padding_px = config.window.effective_padding_px();
        self.window.max_border_width_px = config.window.max_border_width_px();
        self.environment.dim = 0.0;
        let uniform = FloorUniform::from_config(
            config,
            self.environment.diffuse_irradiance,
            self.environment.background_exposure,
        );
        self.write_floor_uniform(&uniform)?;
        Ok(())
    }

    pub fn set_background_exposure(&mut self, exposure: f32, config: &AppConfig) -> Result<()> {
        ensure!(
            exposure.is_finite() && (0.0..=1.0).contains(&exposure),
            "background exposure fade must be between 0 and 1"
        );
        if self.environment.background_exposure == exposure {
            return Ok(());
        }
        self.environment.background_exposure = exposure;
        let uniform = FloorUniform::from_config(
            config,
            self.environment.diffuse_irradiance,
            self.environment.background_exposure * (1.0 - self.environment.dim),
        );
        self.write_floor_uniform(&uniform)
    }

    fn write_floor_uniform(&self, uniform: &FloorUniform) -> Result<()> {
        unsafe {
            let mapped = self.device.map_memory(
                self.floor.memory,
                0,
                std::mem::size_of::<FloorUniform>() as u64,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(
                (uniform as *const FloorUniform).cast::<u8>(),
                mapped.cast::<u8>(),
                std::mem::size_of::<FloorUniform>(),
            );
            self.device.unmap_memory(self.floor.memory);
        }
        Ok(())
    }

    pub fn update_skybox_diffuse(
        &mut self,
        diffuse_irradiance: Vec3,
        config: &AppConfig,
    ) -> Result<()> {
        self.environment.diffuse_irradiance = diffuse_irradiance;
        self.update_config(config)
    }
}

impl Drop for SceneRenderer {
    fn drop(&mut self) {
        unsafe {
            for pipeline in self.environment_pipelines {
                self.device.destroy_pipeline(pipeline, None);
            }
            self.device.destroy_pipeline(self.cursor_pipeline, None);
            self.device.destroy_pipeline(self.window_pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            if self.floor.pool != vk::DescriptorPool::null() {
                self.device.destroy_descriptor_pool(self.floor.pool, None);
            }
            if self.floor.buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.floor.buffer, None);
            }
            if self.floor.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.floor.memory, None);
            }
            if self.environment.pool != vk::DescriptorPool::null() {
                self.device
                    .destroy_descriptor_pool(self.environment.pool, None);
            }
            if self.environment.buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.environment.buffer, None);
            }
            if self.environment.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.environment.memory, None);
            }
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_sampler(self.sky_sampler, None);
            if self.floor_descriptor_layout != vk::DescriptorSetLayout::null() {
                self.device
                    .destroy_descriptor_set_layout(self.floor_descriptor_layout, None);
            }
            if self.environment_descriptor_layout != vk::DescriptorSetLayout::null() {
                self.device
                    .destroy_descriptor_set_layout(self.environment_descriptor_layout, None);
            }
            self.device
                .destroy_descriptor_set_layout(self.descriptor_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

#[cfg(test)]
#[path = "scene/tests.rs"]
mod tests;
