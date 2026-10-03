use anyhow::{Context, Result, ensure};
use ash::vk;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use half::f16;
use openxr as xr;
use std::{
    fs::{self, File},
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    config::AppConfig,
    gpu::SharedImage,
    panel::{PanelGeometry, PanelPose},
};

#[cfg(test)]
pub(crate) const FALLBACK_FLOOR_Y: f32 = spacetop_config::FALLBACK_FLOOR_HEIGHT;

const SHADER: &str = include_str!("scene.wgsl");
static NEXT_PANEL_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

fn shader(
    entry: &str,
    stage: naga::ShaderStage,
    trace_through_transparent_windows: bool,
) -> Result<Vec<u32>> {
    let shader_source = SHADER.replace(
        "const TRACE_THROUGH_TRANSPARENT_WINDOWS: bool = false;",
        &format!(
            "const TRACE_THROUGH_TRANSPARENT_WINDOWS: bool = {};",
            trace_through_transparent_windows
        ),
    );
    let module = naga::front::wgsl::parse_str(&shader_source)
        .map_err(|error| anyhow::anyhow!(error.emit_to_string(&shader_source)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::IMMEDIATES
            | naga::valid::Capabilities::TEXTURE_AND_SAMPLER_BINDING_ARRAY
            | naga::valid::Capabilities::TEXTURE_AND_SAMPLER_BINDING_ARRAY_NON_UNIFORM_INDEXING,
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

pub(crate) struct SkyboxTexture {
    device: ash::Device,
    extent: vk::Extent2D,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    staging: Option<(vk::Buffer, vk::DeviceMemory)>,
}

impl SkyboxTexture {
    pub fn new(
        renderer: &SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        image: &str,
    ) -> Result<Self> {
        let path = background_path(image)?;
        let pixels = image::ImageReader::open(&path)
            .with_context(|| format!("open skybox {}", path.display()))?
            .with_guessed_format()
            .context("identify skybox image format")?
            .decode()
            .with_context(|| format!("decode skybox {}", path.display()))?
            .into_rgba32f();
        let extent = vk::Extent2D {
            width: pixels.width(),
            height: pixels.height(),
        };
        ensure!(extent.width > 0 && extent.height > 0, "empty skybox image");
        let pixels = pixels
            .into_raw()
            .into_iter()
            .map(f16::from_f32)
            .collect::<Vec<_>>();
        let upload_size = std::mem::size_of_val(pixels.as_slice()) as u64;
        let device = &renderer.device;
        let mut skybox = Self {
            device: device.clone(),
            extent,
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            view: vk::ImageView::null(),
            staging: None,
        };
        unsafe {
            skybox.image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R16G16B16A16_SFLOAT)
                    .extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED),
                None,
            )?;
            let requirements = device.get_image_memory_requirements(skybox.image);
            skybox.memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type(
                        instance,
                        physical_device,
                        requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )?),
                None,
            )?;
            device.bind_image_memory(skybox.image, skybox.memory, 0)?;
            skybox.view = image_view(
                device,
                skybox.image,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageAspectFlags::COLOR,
            )?;
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(upload_size)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                None,
            )?;
            let buffer_requirements = device.get_buffer_memory_requirements(buffer);
            let upload_memory_type = match memory_type(
                instance,
                physical_device,
                buffer_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            ) {
                Ok(memory_type) => memory_type,
                Err(error) => {
                    device.destroy_buffer(buffer, None);
                    return Err(error);
                }
            };
            let buffer_memory = match device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(buffer_requirements.size)
                    .memory_type_index(upload_memory_type),
                None,
            ) {
                Ok(memory) => memory,
                Err(error) => {
                    device.destroy_buffer(buffer, None);
                    return Err(error.into());
                }
            };
            if let Err(error) = device.bind_buffer_memory(buffer, buffer_memory, 0) {
                device.destroy_buffer(buffer, None);
                device.free_memory(buffer_memory, None);
                return Err(error.into());
            }
            let mapped =
                match device.map_memory(buffer_memory, 0, upload_size, vk::MemoryMapFlags::empty())
                {
                    Ok(mapped) => mapped,
                    Err(error) => {
                        device.destroy_buffer(buffer, None);
                        device.free_memory(buffer_memory, None);
                        return Err(error.into());
                    }
                };
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr().cast::<u8>(),
                mapped.cast::<u8>(),
                upload_size as usize,
            );
            device.unmap_memory(buffer_memory);
            skybox.staging = Some((buffer, buffer_memory));
        }
        eprintln!("Skybox: {}", path.display());
        Ok(skybox)
    }

    pub fn needs_upload(&self) -> bool {
        self.staging.is_some()
    }

    pub unsafe fn upload(&self, command: vk::CommandBuffer) {
        let Some((buffer, _)) = self.staging else {
            return;
        };
        let range = image_range(vk::ImageAspectFlags::COLOR);
        let to_transfer = vk::ImageMemoryBarrier::default()
            .image(self.image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        unsafe {
            self.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            self.device.cmd_copy_buffer_to_image(
                command,
                buffer,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: self.extent.width,
                        height: self.extent.height,
                        depth: 1,
                    })],
            );
            let to_shader = vk::ImageMemoryBarrier::default()
                .image(self.image)
                .subresource_range(range)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ);
            self.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_shader],
            );
        }
    }

    pub fn upload_complete(&mut self) {
        if let Some((buffer, memory)) = self.staging.take() {
            unsafe {
                self.device.destroy_buffer(buffer, None);
                self.device.free_memory(memory, None);
            }
        }
    }
}

impl Drop for SkyboxTexture {
    fn drop(&mut self) {
        unsafe {
            if let Some((buffer, memory)) = self.staging.take() {
                self.device.destroy_buffer(buffer, None);
                self.device.free_memory(memory, None);
            }
            if self.view != vk::ImageView::null() {
                self.device.destroy_image_view(self.view, None);
            }
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

fn image_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
) -> Result<vk::ImageView> {
    Ok(unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(image_range(aspect)),
            None,
        )
    }?)
}

pub(crate) struct PanelTexture {
    device: ash::Device,
    id: u64,
    pub shared: SharedImage,
    view: vk::ImageView,
    pool: vk::DescriptorPool,
    descriptor: vk::DescriptorSet,
}

impl PanelTexture {
    pub fn new(renderer: &SceneRenderer, shared: SharedImage) -> Result<Self> {
        let device = &renderer.device;
        let id = loop {
            let current = NEXT_PANEL_TEXTURE_ID.load(Ordering::Relaxed);
            let next = current
                .checked_add(1)
                .context("exhausted panel texture identifiers")?;
            if NEXT_PANEL_TEXTURE_ID
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break current;
            }
        };
        let mut texture = Self {
            device: device.clone(),
            id,
            shared,
            view: vk::ImageView::null(),
            pool: vk::DescriptorPool::null(),
            descriptor: vk::DescriptorSet::null(),
        };
        texture.view = image_view(
            device,
            texture.shared.image,
            vk::Format::R8G8B8A8_SRGB,
            vk::ImageAspectFlags::COLOR,
        )?;
        let sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: 1,
            },
        ];
        unsafe {
            texture.pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?;
            texture.descriptor = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(texture.pool)
                    .set_layouts(&[renderer.descriptor_layout]),
            )?[0];
            let images = [vk::DescriptorImageInfo::default()
                .image_view(texture.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let samplers = [vk::DescriptorImageInfo::default().sampler(renderer.sampler)];
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(texture.descriptor)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&images),
                    vk::WriteDescriptorSet::default()
                        .dst_set(texture.descriptor)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&samplers),
                ],
                &[],
            );
        }
        Ok(texture)
    }

    pub unsafe fn ownership(&self, command: vk::CommandBuffer, queue_family: u32, acquire: bool) {
        let (
            old,
            new,
            source,
            destination,
            source_access,
            destination_access,
            source_stage,
            destination_stage,
        ) = if acquire {
            (
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::QUEUE_FAMILY_FOREIGN_EXT,
                queue_family,
                vk::AccessFlags::empty(),
                vk::AccessFlags::SHADER_READ,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
        } else {
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::GENERAL,
                queue_family,
                vk::QUEUE_FAMILY_FOREIGN_EXT,
                vk::AccessFlags::SHADER_READ,
                vk::AccessFlags::empty(),
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            )
        };
        let barrier = vk::ImageMemoryBarrier::default()
            .image(self.shared.image)
            .subresource_range(image_range(vk::ImageAspectFlags::COLOR))
            .old_layout(old)
            .new_layout(new)
            .src_queue_family_index(source)
            .dst_queue_family_index(destination)
            .src_access_mask(source_access)
            .dst_access_mask(destination_access);
        unsafe {
            self.device.cmd_pipeline_barrier(
                command,
                source_stage,
                destination_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }
}

impl Drop for PanelTexture {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_image_view(self.view, None);
        }
    }
}

pub(crate) struct RenderTarget {
    device: ash::Device,
    pub extent: vk::Extent2D,
    color_view: vk::ImageView,
    depth: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    framebuffer: vk::Framebuffer,
}

impl RenderTarget {
    pub fn new(renderer: &SceneRenderer, image: vk::Image, extent: vk::Extent2D) -> Result<Self> {
        let device = &renderer.device;
        let mut target = Self {
            device: device.clone(),
            extent,
            color_view: vk::ImageView::null(),
            depth: vk::Image::null(),
            depth_memory: vk::DeviceMemory::null(),
            depth_view: vk::ImageView::null(),
            framebuffer: vk::Framebuffer::null(),
        };
        target.color_view =
            image_view(device, image, renderer.format, vk::ImageAspectFlags::COLOR)?;
        unsafe {
            target.depth = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::D32_SFLOAT)
                    .extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT),
                None,
            )?;
            let requirements = device.get_image_memory_requirements(target.depth);
            target.depth_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(requirements.memory_type_bits.trailing_zeros()),
                None,
            )?;
            device.bind_image_memory(target.depth, target.depth_memory, 0)?;
            target.depth_view = image_view(
                device,
                target.depth,
                vk::Format::D32_SFLOAT,
                vk::ImageAspectFlags::DEPTH,
            )?;
            target.framebuffer = device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(renderer.render_pass)
                    .attachments(&[target.color_view, target.depth_view])
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )?;
        }
        Ok(target)
    }
}

impl Drop for RenderTarget {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_framebuffer(self.framebuffer, None);
            self.device.destroy_image_view(self.depth_view, None);
            self.device.destroy_image(self.depth, None);
            self.device.free_memory(self.depth_memory, None);
            self.device.destroy_image_view(self.color_view, None);
        }
    }
}

pub(crate) struct SceneRenderer {
    device: ash::Device,
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
    render_pass: vk::RenderPass,
    descriptor_layout: vk::DescriptorSetLayout,
    floor_descriptor_layout: vk::DescriptorSetLayout,
    environment_descriptor_layout: vk::DescriptorSetLayout,
    floor_pool: vk::DescriptorPool,
    floor_descriptor: vk::DescriptorSet,
    floor_buffer: vk::Buffer,
    floor_memory: vk::DeviceMemory,
    environment_pool: vk::DescriptorPool,
    environment_descriptor: vk::DescriptorSet,
    environment_buffer: vk::Buffer,
    environment_memory: vk::DeviceMemory,
    environment_buffer_size: u64,
    environment_descriptor_capacity: u32,
    environment_panel_ids: Vec<u64>,
    environment_skybox_view: vk::ImageView,
    max_environment_windows: u32,
    sampler: vk::Sampler,
    sky_sampler: vk::Sampler,
    layout: vk::PipelineLayout,
    window_pipeline: vk::Pipeline,
    cursor_pipeline: vk::Pipeline,
    environment_first_hit_pipeline: vk::Pipeline,
    environment_transparent_pipeline: vk::Pipeline,
    trace_through_transparent_windows: bool,
}

pub(crate) struct SceneFrame<'a> {
    pub skybox: Option<&'a SkyboxTexture>,
    pub panels: &'a [(&'a PanelTexture, PanelGeometry)],
    pub cursor: Option<PanelPose>,
    pub cursor_close_panel: Option<(PanelGeometry, Vec2)>,
    pub grabbed_panel: Option<PanelGeometry>,
    pub floor_y: f32,
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
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct WindowGpuData {
    center_width: [f32; 4],
    right_height: [f32; 4],
    up: [f32; 4],
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct WindowBufferHeader {
    count: u32,
    padding: [u32; 3],
}

impl From<&AppConfig> for FloorUniform {
    fn from(config: &AppConfig) -> Self {
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
                0.0,
                0.0,
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
            device: device.clone(),
            instance: instance.clone(),
            physical_device,
            format,
            render_pass: vk::RenderPass::null(),
            descriptor_layout: vk::DescriptorSetLayout::null(),
            floor_descriptor_layout: vk::DescriptorSetLayout::null(),
            environment_descriptor_layout: vk::DescriptorSetLayout::null(),
            floor_pool: vk::DescriptorPool::null(),
            floor_descriptor: vk::DescriptorSet::null(),
            floor_buffer: vk::Buffer::null(),
            floor_memory: vk::DeviceMemory::null(),
            environment_pool: vk::DescriptorPool::null(),
            environment_descriptor: vk::DescriptorSet::null(),
            environment_buffer: vk::Buffer::null(),
            environment_memory: vk::DeviceMemory::null(),
            environment_buffer_size: 0,
            environment_descriptor_capacity: 0,
            environment_panel_ids: Vec::new(),
            environment_skybox_view: vk::ImageView::null(),
            max_environment_windows: 0,
            sampler: vk::Sampler::null(),
            sky_sampler: vk::Sampler::null(),
            layout: vk::PipelineLayout::null(),
            window_pipeline: vk::Pipeline::null(),
            cursor_pipeline: vk::Pipeline::null(),
            environment_first_hit_pipeline: vk::Pipeline::null(),
            environment_transparent_pipeline: vk::Pipeline::null(),
            trace_through_transparent_windows: config.floor.trace_through_transparent_windows,
        };
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
            renderer.max_environment_windows = limits
                .max_per_stage_descriptor_sampled_images
                .min(limits.max_descriptor_set_sampled_images)
                .saturating_sub(2)
                .min(
                    limits
                        .max_storage_buffer_range
                        .saturating_sub(std::mem::size_of::<WindowBufferHeader>() as u32)
                        / std::mem::size_of::<WindowGpuData>() as u32,
                );
            ensure!(
                renderer.max_environment_windows > 0,
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
                    .descriptor_count(renderer.max_environment_windows)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let binding_flags = [
                vk::DescriptorBindingFlags::empty(),
                vk::DescriptorBindingFlags::empty(),
                vk::DescriptorBindingFlags::empty(),
                vk::DescriptorBindingFlags::empty(),
                vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT,
            ];
            let mut binding_flags_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
                .binding_flags(&binding_flags);
            renderer.environment_descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(&environment_bindings)
                    .push_next(&mut binding_flags_info),
                None,
            )?;
            let uniform = FloorUniform::from(config);
            renderer.floor_buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(std::mem::size_of::<FloorUniform>() as u64)
                    .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                None,
            )?;
            let requirements = device.get_buffer_memory_requirements(renderer.floor_buffer);
            renderer.floor_memory = device.allocate_memory(
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
            device.bind_buffer_memory(renderer.floor_buffer, renderer.floor_memory, 0)?;
            let mapped = device.map_memory(
                renderer.floor_memory,
                0,
                std::mem::size_of::<FloorUniform>() as u64,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(
                (&uniform as *const FloorUniform).cast::<u8>(),
                mapped.cast::<u8>(),
                std::mem::size_of::<FloorUniform>(),
            );
            device.unmap_memory(renderer.floor_memory);
            renderer.floor_pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::UNIFORM_BUFFER,
                        descriptor_count: 1,
                    }]),
                None,
            )?;
            renderer.floor_descriptor = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(renderer.floor_pool)
                    .set_layouts(&[renderer.floor_descriptor_layout]),
            )?[0];
            let buffers = [vk::DescriptorBufferInfo::default()
                .buffer(renderer.floor_buffer)
                .range(std::mem::size_of::<FloorUniform>() as u64)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(renderer.floor_descriptor)
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
                    .address_mode_u(vk::SamplerAddressMode::REPEAT)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
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
        renderer.window_pipeline = renderer.pipeline("window", false)?;
        renderer.cursor_pipeline = renderer.pipeline("cursor", false)?;
        renderer.environment_first_hit_pipeline = renderer.pipeline("environment", false)?;
        renderer.environment_transparent_pipeline = renderer.pipeline("environment", true)?;
        Ok(renderer)
    }

    pub fn update_config(&mut self, config: &AppConfig) -> Result<()> {
        self.trace_through_transparent_windows = config.floor.trace_through_transparent_windows;
        let uniform = FloorUniform::from(config);
        unsafe {
            let mapped = self.device.map_memory(
                self.floor_memory,
                0,
                std::mem::size_of::<FloorUniform>() as u64,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(
                (&uniform as *const FloorUniform).cast::<u8>(),
                mapped.cast::<u8>(),
                std::mem::size_of::<FloorUniform>(),
            );
            self.device.unmap_memory(self.floor_memory);
        }
        Ok(())
    }

    pub fn prepare_frame(&mut self, frame: &SceneFrame<'_>) -> Result<()> {
        let skybox = frame.skybox.context("environment pass requires a skybox")?;
        let window_count = u32::try_from(frame.panels.len())
            .context("too many windows for Vulkan descriptor indexing")?;
        ensure!(
            window_count <= self.max_environment_windows,
            "window count {window_count} exceeds this GPU's reflected-window capacity {}",
            self.max_environment_windows
        );
        let descriptor_count = window_count.max(1);
        let descriptor_changed = descriptor_count > self.environment_descriptor_capacity;
        if descriptor_changed {
            unsafe {
                if self.environment_pool != vk::DescriptorPool::null() {
                    self.device
                        .destroy_descriptor_pool(self.environment_pool, None);
                    self.environment_pool = vk::DescriptorPool::null();
                    self.environment_descriptor = vk::DescriptorSet::null();
                }
                self.environment_pool = self.device.create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&[
                            vk::DescriptorPoolSize {
                                ty: vk::DescriptorType::STORAGE_BUFFER,
                                descriptor_count: 1,
                            },
                            vk::DescriptorPoolSize {
                                ty: vk::DescriptorType::SAMPLER,
                                descriptor_count: 2,
                            },
                            vk::DescriptorPoolSize {
                                ty: vk::DescriptorType::SAMPLED_IMAGE,
                                descriptor_count: descriptor_count + 1,
                            },
                        ]),
                    None,
                )?;
                let counts = [descriptor_count];
                let mut variable_count =
                    vk::DescriptorSetVariableDescriptorCountAllocateInfo::default()
                        .descriptor_counts(&counts);
                self.environment_descriptor = self.device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.environment_pool)
                        .set_layouts(&[self.environment_descriptor_layout])
                        .push_next(&mut variable_count),
                )?[0];
                self.environment_descriptor_capacity = descriptor_count;
                self.environment_panel_ids.clear();
                self.environment_skybox_view = vk::ImageView::null();
            }
        }

        let required_buffer_size = std::mem::size_of::<WindowBufferHeader>() as u64
            + frame.panels.len() as u64 * std::mem::size_of::<WindowGpuData>() as u64;
        let mut buffer_changed = false;
        if required_buffer_size > self.environment_buffer_size {
            let (buffer, memory, allocation_size) = unsafe {
                let buffer = self.device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(required_buffer_size)
                        .usage(vk::BufferUsageFlags::STORAGE_BUFFER),
                    None,
                )?;
                let requirements = self.device.get_buffer_memory_requirements(buffer);
                let memory_type_index = match memory_type(
                    &self.instance,
                    self.physical_device,
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                ) {
                    Ok(index) => index,
                    Err(error) => {
                        self.device.destroy_buffer(buffer, None);
                        return Err(error);
                    }
                };
                let memory = match self.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type_index),
                    None,
                ) {
                    Ok(memory) => memory,
                    Err(error) => {
                        self.device.destroy_buffer(buffer, None);
                        return Err(error.into());
                    }
                };
                if let Err(error) = self.device.bind_buffer_memory(buffer, memory, 0) {
                    self.device.destroy_buffer(buffer, None);
                    self.device.free_memory(memory, None);
                    return Err(error.into());
                }
                (buffer, memory, required_buffer_size)
            };
            unsafe {
                if self.environment_buffer != vk::Buffer::null() {
                    self.device.destroy_buffer(self.environment_buffer, None);
                    self.device.free_memory(self.environment_memory, None);
                }
            }
            self.environment_buffer = buffer;
            self.environment_memory = memory;
            self.environment_buffer_size = allocation_size;
            buffer_changed = true;
        }

        let windows = frame
            .panels
            .iter()
            .map(|(_, geometry)| {
                let pose = geometry.pose;
                let right = pose.orientation() * Vec3::X;
                let up = pose.orientation() * Vec3::Y;
                let height =
                    pose.width_m * geometry.logical_size.h as f32 / geometry.logical_size.w as f32;
                WindowGpuData {
                    center_width: [pose.center.x, pose.center.y, pose.center.z, pose.width_m],
                    right_height: [right.x, right.y, right.z, height],
                    up: [up.x, up.y, up.z, 0.0],
                }
            })
            .collect::<Vec<_>>();
        let header = WindowBufferHeader {
            count: window_count,
            padding: [0; 3],
        };
        unsafe {
            let mapped = self.device.map_memory(
                self.environment_memory,
                0,
                required_buffer_size,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(
                (&header as *const WindowBufferHeader).cast::<u8>(),
                mapped.cast::<u8>(),
                std::mem::size_of::<WindowBufferHeader>(),
            );
            std::ptr::copy_nonoverlapping(
                windows.as_ptr().cast::<u8>(),
                mapped
                    .cast::<u8>()
                    .add(std::mem::size_of::<WindowBufferHeader>()),
                windows.len() * std::mem::size_of::<WindowGpuData>(),
            );
            self.device.unmap_memory(self.environment_memory);

            let panel_ids = frame
                .panels
                .iter()
                .map(|(texture, _)| texture.id)
                .collect::<Vec<_>>();
            let skybox_changed = skybox.view != self.environment_skybox_view;
            if descriptor_changed {
                let filtering = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
                let skybox_filtering =
                    [vk::DescriptorImageInfo::default().sampler(self.sky_sampler)];
                self.device.update_descriptor_sets(
                    &[
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.environment_descriptor)
                            .dst_binding(1)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&filtering),
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.environment_descriptor)
                            .dst_binding(3)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&skybox_filtering),
                    ],
                    &[],
                );
            }
            if descriptor_changed || buffer_changed {
                let window_buffer = [vk::DescriptorBufferInfo::default()
                    .buffer(self.environment_buffer)
                    .range(required_buffer_size)];
                self.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.environment_descriptor)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(&window_buffer)],
                    &[],
                );
            }
            if descriptor_changed || skybox_changed {
                let skybox_image = [vk::DescriptorImageInfo::default()
                    .image_view(skybox.view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                self.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.environment_descriptor)
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&skybox_image)],
                    &[],
                );
                self.environment_skybox_view = skybox.view;
            }
            if descriptor_changed || skybox_changed || panel_ids != self.environment_panel_ids {
                let window_images = frame
                    .panels
                    .iter()
                    .map(|(texture, _)| {
                        vk::DescriptorImageInfo::default()
                            .image_view(texture.view)
                            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    })
                    .chain(std::iter::repeat_n(
                        vk::DescriptorImageInfo::default()
                            .image_view(skybox.view)
                            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                        self.environment_descriptor_capacity as usize - panel_ids.len(),
                    ))
                    .collect::<Vec<_>>();
                self.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.environment_descriptor)
                        .dst_binding(4)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&window_images)],
                    &[],
                );
                self.environment_panel_ids = panel_ids;
            }
        }
        Ok(())
    }

    fn pipeline(
        &self,
        fragment: &str,
        trace_through_transparent_windows: bool,
    ) -> Result<vk::Pipeline> {
        let vertex_name = if fragment == "environment" {
            c"sky_vertex"
        } else {
            c"vertex"
        };
        let vertex_code = shader(
            vertex_name.to_str()?,
            naga::ShaderStage::Vertex,
            trace_through_transparent_windows,
        )?;
        let fragment_code = shader(
            fragment,
            naga::ShaderStage::Fragment,
            trace_through_transparent_windows,
        )?;
        let vertex = unsafe {
            self.device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&vertex_code),
                None,
            )
        }?;
        let result = (|| -> Result<_> {
            let fragment_module = unsafe {
                self.device.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&fragment_code),
                    None,
                )
            }?;
            let fragment_name = std::ffi::CString::new(fragment)?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vertex)
                    .name(vertex_name),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(fragment_module)
                    .name(&fragment_name),
            ];
            let input = vk::PipelineVertexInputStateCreateInfo::default();
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .line_width(1.0);
            let samples = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(fragment != "environment")
                .depth_write_enable(fragment == "window")
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
            let alpha_blend = fragment == "floor_albedo";
            let attachments = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(fragment != "environment")
                .src_color_blend_factor(if alpha_blend {
                    vk::BlendFactor::SRC_ALPHA
                } else {
                    vk::BlendFactor::ONE
                })
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
            let dynamic = vk::PipelineDynamicStateCreateInfo::default()
                .dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]);
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&samples)
                .depth_stencil_state(&depth)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(self.layout)
                .render_pass(self.render_pass);
            let result = unsafe {
                self.device
                    .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
            };
            unsafe {
                self.device.destroy_shader_module(fragment_module, None);
            }
            match result {
                Ok(pipelines) => Ok(pipelines[0]),
                Err((pipelines, error)) => {
                    for pipeline in pipelines {
                        unsafe {
                            self.device.destroy_pipeline(pipeline, None);
                        }
                    }
                    Err(error.into())
                }
            }
        })();
        unsafe {
            self.device.destroy_shader_module(vertex, None);
        }
        result.context("create Vulkan scene pipeline")
    }

    pub unsafe fn draw<'a>(
        &self,
        command: vk::CommandBuffer,
        target: &RenderTarget,
        view: &xr::View,
        frame: &SceneFrame<'a>,
    ) {
        let projection = view_projection(view);
        let mut panels = frame.panels.to_vec();
        panels.sort_by(|(_, first), (_, second)| {
            let first = (projection * first.pose.center.extend(1.0)).w;
            let second = (projection * second.pose.center.extend(1.0)).w;
            second.total_cmp(&first)
        });
        let area = vk::Rect2D::default().extent(target.extent);
        let clear = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        unsafe {
            self.device.cmd_begin_render_pass(
                command,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(target.framebuffer)
                    .render_area(area)
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                command,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: target.extent.width as f32,
                    height: target.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(command, 0, &[area]);
            self.device.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                1,
                &[self.floor_descriptor, self.environment_descriptor],
                &[],
            );
            let floor_height = [frame.floor_y];
            let floor_height_bytes =
                std::slice::from_raw_parts(floor_height.as_ptr().cast::<u8>(), 4);
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                108,
                floor_height_bytes,
            );
            let mut eye = [
                view.pose.position.x,
                view.pose.position.y,
                view.pose.position.z,
                0.0,
            ];
            let eye_bytes = std::slice::from_raw_parts(eye.as_ptr().cast::<u8>(), 16);
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                112,
                eye_bytes,
            );
            self.device.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                if self.trace_through_transparent_windows {
                    self.environment_transparent_pipeline
                } else {
                    self.environment_first_hit_pipeline
                },
            );
            self.draw_panel(command, sky_matrix(view));
            self.device.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.window_pipeline,
            );
            for (texture, geometry) in panels {
                let close_hit = frame
                    .cursor_close_panel
                    .filter(|(close_geometry, _)| *close_geometry == geometry);
                let cursor_position =
                    close_hit.map_or([0.0; 4], |(_, position)| [position.x, position.y, 0.0, 0.0]);
                let cursor_position_bytes =
                    std::slice::from_raw_parts(cursor_position.as_ptr().cast::<u8>(), 16);
                self.device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    64,
                    cursor_position_bytes,
                );
                eye[0] = geometry.logical_size.w as f32;
                eye[1] = geometry.logical_size.h as f32;
                eye[2] = if frame.grabbed_panel == Some(geometry) {
                    1.0
                } else {
                    0.0
                };
                eye[3] = if close_hit.is_some() { 1.0 } else { 0.0 };
                let eye_bytes = std::slice::from_raw_parts(eye.as_ptr().cast::<u8>(), 16);
                self.device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    112,
                    eye_bytes,
                );
                self.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[texture.descriptor],
                    &[],
                );
                self.draw_panel(command, projection * model(geometry));
            }
            if let Some(mut pose) = frame.cursor {
                pose.center += pose.orientation() * Vec3::Z * 0.001;
                self.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.cursor_pipeline,
                );
                self.draw_panel(
                    command,
                    projection
                        * model(PanelGeometry {
                            pose,
                            logical_size: (21, 21).into(),
                        }),
                );
            }
            self.device.cmd_end_render_pass(command);
        }
    }

    unsafe fn draw_panel(&self, command: vk::CommandBuffer, matrix: Mat4) {
        let columns = matrix.to_cols_array();
        let bytes = unsafe { std::slice::from_raw_parts(columns.as_ptr().cast::<u8>(), 64) };
        unsafe {
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes,
            );
            self.device.cmd_draw(command, 6, 1, 0, 0);
        }
    }
}

impl Drop for SceneRenderer {
    fn drop(&mut self) {
        unsafe {
            self.device
                .destroy_pipeline(self.environment_first_hit_pipeline, None);
            self.device
                .destroy_pipeline(self.environment_transparent_pipeline, None);
            self.device.destroy_pipeline(self.cursor_pipeline, None);
            self.device.destroy_pipeline(self.window_pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            if self.floor_pool != vk::DescriptorPool::null() {
                self.device.destroy_descriptor_pool(self.floor_pool, None);
            }
            if self.floor_buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.floor_buffer, None);
            }
            if self.floor_memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.floor_memory, None);
            }
            if self.environment_pool != vk::DescriptorPool::null() {
                self.device
                    .destroy_descriptor_pool(self.environment_pool, None);
            }
            if self.environment_buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.environment_buffer, None);
            }
            if self.environment_memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.environment_memory, None);
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
mod tests {
    use super::*;

    #[test]
    fn shaders_compile() {
        for (entry, stage) in [
            ("sky_vertex", naga::ShaderStage::Vertex),
            ("vertex", naga::ShaderStage::Vertex),
            ("window", naga::ShaderStage::Fragment),
            ("cursor", naga::ShaderStage::Fragment),
            ("environment", naga::ShaderStage::Fragment),
        ] {
            assert_eq!(shader(entry, stage, false).unwrap()[0], 0x0723_0203);
        }
        assert_eq!(
            shader("environment", naga::ShaderStage::Fragment, true).unwrap()[0],
            0x0723_0203
        );
    }

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
}
