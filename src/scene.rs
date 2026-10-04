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
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
};

use crate::{
    config::AppConfig,
    gpu::SharedImage,
    panel::{PanelGeometry, PanelPose},
};

const SHADER_PARTS: [&str; 5] = [
    include_str!("shaders/shared.wgsl"),
    include_str!("shaders/panel.wgsl"),
    include_str!("shaders/tone_mapping.wgsl"),
    include_str!("shaders/lighting.wgsl"),
    include_str!("shaders/environment.wgsl"),
];
static NEXT_PANEL_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

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

#[derive(Clone, Copy)]
struct SkyboxMip {
    width: u32,
    height: u32,
    buffer_offset: u64,
}

fn build_skybox_mips(width: u32, height: u32, mut pixels: Vec<f16>) -> (Vec<f16>, Vec<SkyboxMip>) {
    let mut mips = vec![SkyboxMip {
        width,
        height,
        buffer_offset: 0,
    }];
    let mut previous_width = width;
    let mut previous_height = height;

    while previous_width > 1 || previous_height > 1 {
        let next_width = (previous_width / 2).max(1);
        let next_height = (previous_height / 2).max(1);
        let previous = *mips.last().expect("base skybox mip exists");
        let previous_offset = previous.buffer_offset as usize / std::mem::size_of::<f16>();
        let previous_len = (previous.width * previous.height * 4) as usize;
        let next = downsample_skybox_mip(
            &pixels[previous_offset..previous_offset + previous_len],
            previous.width,
            previous.height,
            next_width,
            next_height,
        );
        let buffer_offset = std::mem::size_of_val(pixels.as_slice()) as u64;
        pixels.extend(next);
        mips.push(SkyboxMip {
            width: next_width,
            height: next_height,
            buffer_offset,
        });
        previous_width = next_width;
        previous_height = next_height;
    }

    (pixels, mips)
}

fn downsample_skybox_mip(
    source: &[f16],
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
) -> Vec<f16> {
    let mut output = vec![f16::ZERO; (width * height * 4) as usize];
    for y in 0..height {
        let source_y_start = y as f32 * source_height as f32 / height as f32;
        let source_y_end = (y + 1) as f32 * source_height as f32 / height as f32;
        for x in 0..width {
            let source_x_start = x as f32 * source_width as f32 / width as f32;
            let source_x_end = (x + 1) as f32 * source_width as f32 / width as f32;
            let mut sum = [0.0; 4];
            let mut total_weight = 0.0;
            for source_y in source_y_start.floor() as u32..source_y_end.ceil() as u32 {
                let y_start = source_y_start.max(source_y as f32);
                let y_end = source_y_end.min(source_y as f32 + 1.0);
                let latitude_weight = (std::f32::consts::PI * y_start / source_height as f32).cos()
                    - (std::f32::consts::PI * y_end / source_height as f32).cos();
                for source_x in source_x_start.floor() as u32..source_x_end.ceil() as u32 {
                    let x_start = source_x_start.max(source_x as f32);
                    let x_end = source_x_end.min(source_x as f32 + 1.0);
                    let weight = (x_end - x_start) * latitude_weight;
                    let source_index = ((source_y * source_width + source_x) * 4) as usize;
                    for channel in 0..4 {
                        sum[channel] += source[source_index + channel].to_f32() * weight;
                    }
                    total_weight += weight;
                }
            }
            let destination_index = ((y * width + x) * 4) as usize;
            for channel in 0..4 {
                output[destination_index + channel] = f16::from_f32(sum[channel] / total_weight);
            }
        }
    }
    output
}

fn integrate_skybox_diffuse(width: u32, height: u32, pixels: &[Vec4]) -> Vec3 {
    let width = width as usize;
    let longitude_step = 2.0 * std::f32::consts::PI / width as f32;
    let upper_half_end = height as f32 * 0.5;
    let irradiance: Vec3 = (0..height.div_ceil(2) as usize)
        .into_par_iter()
        .map(|y| {
            let theta_start = std::f32::consts::PI * y as f32 / height as f32;
            let theta_end =
                std::f32::consts::PI * ((y + 1) as f32).min(upper_half_end) / height as f32;
            let weight =
                longitude_step * 0.5 * (theta_end.sin().powi(2) - theta_start.sin().powi(2));
            let row_sum: Vec3 = pixels[y * width..(y + 1) * width]
                .iter()
                .copied()
                .map(|p| p.xyz())
                .sum();
            row_sum * weight
        })
        .sum();
    irradiance / std::f32::consts::PI
}

pub(crate) struct SkyboxTexture {
    device: ash::Device,
    mips: Vec<SkyboxMip>,
    diffuse_irradiance: Vec3,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    staging: Option<(vk::Buffer, vk::DeviceMemory)>,
}

impl SkyboxTexture {
    #[cfg(test)]
    pub fn new(
        renderer: &SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        image: &str,
    ) -> Result<Self> {
        Self::load(renderer.device.clone(), instance, physical_device, image)
    }

    pub fn load_async(
        renderer: &SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        image: &str,
    ) -> Result<JoinHandle<Result<Self>>> {
        let device = renderer.device.clone();
        let instance = instance.clone();
        let image = image.to_owned();
        Ok(thread::Builder::new()
            .name("spacetop-skybox".into())
            .spawn(move || Self::load(device, &instance, physical_device, &image))?)
    }

    fn load(
        device: ash::Device,
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
        let pixels = pixels.into_raw();
        let diffuse_irradiance = integrate_skybox_diffuse(
            extent.width,
            extent.height,
            bytemuck::cast_slice::<f32, Vec4>(&pixels),
        );
        let pixels = pixels.into_iter().map(f16::from_f32).collect::<Vec<_>>();
        let (pixels, mips) = build_skybox_mips(extent.width, extent.height, pixels);
        let upload_size = std::mem::size_of_val(pixels.as_slice()) as u64;
        let mut skybox = Self {
            device: device.clone(),
            mips,
            diffuse_irradiance,
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
                    .mip_levels(skybox.mips.len() as u32)
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
            skybox.view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(skybox.image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R16G16B16A16_SFLOAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(skybox.mips.len() as u32)
                            .layer_count(1),
                    ),
                None,
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

    pub fn diffuse_irradiance(&self) -> Vec3 {
        self.diffuse_irradiance
    }

    pub unsafe fn upload(&self, command: vk::CommandBuffer) {
        let Some((buffer, _)) = self.staging else {
            return;
        };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(self.mips.len() as u32)
            .layer_count(1);
        let regions = self
            .mips
            .iter()
            .enumerate()
            .map(|(level, mip)| {
                vk::BufferImageCopy::default()
                    .buffer_offset(mip.buffer_offset)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(level as u32)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: mip.width,
                        height: mip.height,
                        depth: 1,
                    })
            })
            .collect::<Vec<_>>();
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
                &regions,
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

fn pack_reflection_atlas(
    sizes: &[vk::Extent2D],
    edge: u32,
) -> Result<(vk::Extent2D, Vec<vk::Rect2D>)> {
    ensure!(
        edge > 0 && edge <= i32::MAX as u32,
        "invalid reflection atlas size"
    );
    ensure!(
        sizes.len() as u64 <= u64::from(edge) * u64::from(edge),
        "too many windows to fit even one texel per window in the {edge}x{edge} reflection atlas"
    );
    let mut order = (0..sizes.len()).collect::<Vec<_>>();
    for size in sizes {
        ensure!(
            size.width > 0 && size.height > 0,
            "window texture dimensions must be nonzero"
        );
    }
    order.sort_by_key(|&index| std::cmp::Reverse(sizes[index].height));
    let pack = |scale: f64| -> Option<Vec<vk::Rect2D>> {
        let mut rects = vec![vk::Rect2D::default(); sizes.len()];
        let (mut x, mut y, mut row_height) = (0, 0, 0);
        for &index in &order {
            let size = vk::Extent2D {
                width: ((f64::from(sizes[index].width) * scale).floor() as u32).max(1),
                height: ((f64::from(sizes[index].height) * scale).floor() as u32).max(1),
            };
            if size.width > edge || size.height > edge {
                return None;
            }
            if x + size.width > edge {
                y += row_height;
                x = 0;
                row_height = 0;
            }
            if y + size.height > edge {
                return None;
            }
            rects[index] = vk::Rect2D {
                offset: vk::Offset2D {
                    x: x as i32,
                    y: y as i32,
                },
                extent: size,
            };
            x += size.width;
            row_height = row_height.max(size.height);
        }
        Some(rects)
    };
    let extent = vk::Extent2D {
        width: edge,
        height: edge,
    };
    if let Some(rects) = pack(1.0) {
        return Ok((extent, rects));
    }
    let mut rects = pack(0.0).context("cannot pack reflection atlas")?;
    let (mut lower, mut upper) = (0.0, 1.0);
    // Retain a proven fit: shelf packing can change discontinuously as rows reflow.
    for _ in 0..32 {
        let scale = (lower + upper) * 0.5;
        if let Some(packed) = pack(scale) {
            rects = packed;
            lower = scale;
        } else {
            upper = scale;
        }
    }
    Ok((extent, rects))
}

fn atlas_dirty_indices(
    previous_ids: &[u64],
    current_ids: &[u64],
    layout_changed: bool,
    atlas_changed: bool,
) -> Vec<usize> {
    if layout_changed || atlas_changed {
        return (0..current_ids.len()).collect();
    }
    current_ids
        .iter()
        .enumerate()
        .filter_map(|(index, id)| (previous_ids.get(index) != Some(id)).then_some(index))
        .collect()
}

struct ReflectionAtlas {
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    extent: vk::Extent2D,
    initialized: bool,
}

impl ReflectionAtlas {
    fn new(renderer: &SceneRenderer, extent: vk::Extent2D) -> Result<Self> {
        let device = &renderer.device;
        let mut atlas = Self {
            device: device.clone(),
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            view: vk::ImageView::null(),
            extent,
            initialized: false,
        };
        unsafe {
            atlas.image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_SRGB)
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
                )
                .context("create reflection atlas image")?;
            let requirements = device.get_image_memory_requirements(atlas.image);
            atlas.memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type(
                            &renderer.instance,
                            renderer.physical_device,
                            requirements.memory_type_bits,
                            vk::MemoryPropertyFlags::DEVICE_LOCAL,
                        )?),
                    None,
                )
                .context("allocate reflection atlas memory")?;
            device.bind_image_memory(atlas.image, atlas.memory, 0)?;
            atlas.view = image_view(
                device,
                atlas.image,
                vk::Format::R8G8B8A8_SRGB,
                vk::ImageAspectFlags::COLOR,
            )?;
        }
        Ok(atlas)
    }
}

impl Drop for ReflectionAtlas {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
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
    environment_skybox_view: vk::ImageView,
    skybox_diffuse_irradiance: Vec3,
    environment_dim: f32,
    max_environment_windows: u32,
    atlas: Option<ReflectionAtlas>,
    atlas_rects: Vec<vk::Rect2D>,
    atlas_panel_ids: Vec<u64>,
    atlas_dirty_indices: Vec<usize>,
    atlas_size: u32,
    sampler: vk::Sampler,
    sky_sampler: vk::Sampler,
    layout: vk::PipelineLayout,
    window_pipeline: vk::Pipeline,
    cursor_pipeline: vk::Pipeline,
    environment_pipelines: [vk::Pipeline; 4],
    trace_through_transparent_windows: bool,
    ambient_occlusion: bool,
    window_padding_px: f32,
    max_border_width_px: f32,
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
    fn from_config(config: &AppConfig, skybox_diffuse_irradiance: Vec3) -> Self {
        let exposure = 2.0_f32.powf(config.background.brightness_stops);
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
            diffuse_irradiance: [
                skybox_diffuse_irradiance.x * exposure,
                skybox_diffuse_irradiance.y * exposure,
                skybox_diffuse_irradiance.z * exposure,
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
            environment_skybox_view: vk::ImageView::null(),
            skybox_diffuse_irradiance: Vec3::ONE,
            environment_dim: 0.0,
            max_environment_windows: 0,
            atlas: None,
            atlas_rects: Vec::new(),
            atlas_panel_ids: Vec::new(),
            atlas_dirty_indices: Vec::new(),
            atlas_size: config.window.reflection_atlas_size.into(),
            sampler: vk::Sampler::null(),
            sky_sampler: vk::Sampler::null(),
            layout: vk::PipelineLayout::null(),
            window_pipeline: vk::Pipeline::null(),
            cursor_pipeline: vk::Pipeline::null(),
            environment_pipelines: [vk::Pipeline::null(); 4],
            trace_through_transparent_windows: config.floor.trace_through_transparent_windows,
            ambient_occlusion: config.floor.ambient_occlusion,
            window_padding_px: config.window.effective_padding_px(),
            max_border_width_px: config.window.max_border_width_px(),
        };
        renderer.validate_atlas_size(renderer.atlas_size)?;
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
            renderer.max_environment_windows = buffer_capacity;
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
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            renderer.environment_descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&environment_bindings),
                None,
            )?;
            let uniform = FloorUniform::from_config(config, renderer.skybox_diffuse_irradiance);
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
        self.atlas_size = atlas_size;
        self.trace_through_transparent_windows = config.floor.trace_through_transparent_windows;
        self.ambient_occlusion = config.floor.ambient_occlusion;
        self.window_padding_px = config.window.effective_padding_px();
        self.max_border_width_px = config.window.max_border_width_px();
        self.environment_dim = 0.0;
        let uniform = FloorUniform::from_config(config, self.skybox_diffuse_irradiance);
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

    pub fn update_skybox_diffuse(
        &mut self,
        diffuse_irradiance: Vec3,
        config: &AppConfig,
    ) -> Result<()> {
        self.skybox_diffuse_irradiance = diffuse_irradiance;
        self.update_config(config)
    }

    pub fn prepare_frame(&mut self, frame: &SceneFrame<'_>) -> Result<()> {
        let skybox = frame.skybox.context("environment pass requires a skybox")?;
        if self.environment_dim != frame.environment_dim {
            let byte_offset =
                std::mem::offset_of!(FloorUniform, ground_radius) + 2 * std::mem::size_of::<f32>();
            unsafe {
                let mapped = self.device.map_memory(
                    self.floor_memory,
                    0,
                    std::mem::size_of::<FloorUniform>() as u64,
                    vk::MemoryMapFlags::empty(),
                )?;
                std::ptr::copy_nonoverlapping(
                    (&frame.environment_dim as *const f32).cast::<u8>(),
                    (mapped.cast::<u8>()).add(byte_offset),
                    std::mem::size_of::<f32>(),
                );
                self.device.unmap_memory(self.floor_memory);
            }
            self.environment_dim = frame.environment_dim;
        }
        let window_count =
            u32::try_from(frame.panels.len()).context("too many windows for Vulkan reflections")?;
        ensure!(
            window_count <= self.max_environment_windows,
            "window count {window_count} exceeds this GPU's reflected-window capacity {}",
            self.max_environment_windows
        );
        let sizes = frame
            .panels
            .iter()
            .map(|(texture, _)| {
                let size = texture.shared.dmabuf.size();
                vk::Extent2D {
                    width: size.w as u32,
                    height: size.h as u32,
                }
            })
            .collect::<Vec<_>>();
        let (extent, rects) = pack_reflection_atlas(&sizes, self.atlas_size)?;
        let atlas_changed = self
            .atlas
            .as_ref()
            .is_none_or(|atlas| atlas.extent != extent);
        let layout_changed = rects != self.atlas_rects;
        if atlas_changed {
            self.atlas = Some(ReflectionAtlas::new(self, extent)?);
        }
        let panel_ids = frame
            .panels
            .iter()
            .map(|(texture, _)| texture.id)
            .collect::<Vec<_>>();
        self.atlas_dirty_indices = atlas_dirty_indices(
            &self.atlas_panel_ids,
            &panel_ids,
            layout_changed,
            atlas_changed,
        );
        self.atlas_rects = rects;
        self.atlas_panel_ids = panel_ids;
        let descriptor_count = 1;
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
                let layouts = [self.environment_descriptor_layout];
                let allocation = vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.environment_pool)
                    .set_layouts(&layouts);
                self.environment_descriptor = self.device.allocate_descriptor_sets(&allocation)?[0];
                self.environment_descriptor_capacity = descriptor_count;
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
            .enumerate()
            .map(|(index, (_, geometry))| {
                let pose = geometry.pose;
                let right = pose.orientation() * Vec3::X;
                let up = pose.orientation() * Vec3::Y;
                let height =
                    pose.width_m * geometry.logical_size.h as f32 / geometry.logical_size.w as f32;
                WindowGpuData {
                    center_width: [pose.center.x, pose.center.y, pose.center.z, pose.width_m],
                    right_height: [right.x, right.y, right.z, height],
                    up: [up.x, up.y, up.z, 0.0],
                    atlas_rect: {
                        let rect = self.atlas_rects[index];
                        [
                            rect.offset.x as f32,
                            rect.offset.y as f32,
                            rect.extent.width as f32,
                            rect.extent.height as f32,
                        ]
                    },
                }
            })
            .collect::<Vec<_>>();
        let header = WindowBufferHeader {
            count: window_count,
            padding: [0; 3],
        };
        let atlas_view = self
            .atlas
            .as_ref()
            .context("reflection atlas was not initialized")?
            .view;
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
            if descriptor_changed || atlas_changed || skybox_changed {
                let window_images = [vk::DescriptorImageInfo::default()
                    .image_view(atlas_view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                self.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.environment_descriptor)
                        .dst_binding(4)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&window_images)],
                    &[],
                );
            }
        }
        Ok(())
    }

    pub unsafe fn copy_reflection_textures(
        &mut self,
        command: vk::CommandBuffer,
        frame: &SceneFrame<'_>,
    ) {
        if self.atlas_dirty_indices.is_empty()
            && self.atlas.as_ref().is_none_or(|atlas| atlas.initialized)
        {
            return;
        }
        let Some(atlas) = &mut self.atlas else {
            return;
        };
        let range = image_range(vk::ImageAspectFlags::COLOR);
        let atlas_barrier = vk::ImageMemoryBarrier::default()
            .image(atlas.image)
            .subresource_range(range)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        unsafe {
            self.device.cmd_pipeline_barrier(
                command,
                if atlas.initialized {
                    vk::PipelineStageFlags::FRAGMENT_SHADER
                } else {
                    vk::PipelineStageFlags::TOP_OF_PIPE
                },
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[atlas_barrier
                    .old_layout(if atlas.initialized {
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                    } else {
                        vk::ImageLayout::UNDEFINED
                    })
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_access_mask(if atlas.initialized {
                        vk::AccessFlags::SHADER_READ
                    } else {
                        vk::AccessFlags::empty()
                    })
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)],
            );
            for &index in &self.atlas_dirty_indices {
                let (texture, _) = &frame.panels[index];
                let rect = &self.atlas_rects[index];
                let source_barrier = vk::ImageMemoryBarrier::default()
                    .image(texture.shared.image)
                    .subresource_range(range)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                self.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[source_barrier
                        .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .src_access_mask(vk::AccessFlags::SHADER_READ)
                        .dst_access_mask(vk::AccessFlags::TRANSFER_READ)],
                );
                let layers = vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1);
                let source_size = texture.shared.dmabuf.size();
                if source_size.w as u32 == rect.extent.width
                    && source_size.h as u32 == rect.extent.height
                {
                    self.device.cmd_copy_image(
                        command,
                        texture.shared.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        atlas.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[vk::ImageCopy::default()
                            .src_subresource(layers)
                            .dst_subresource(layers)
                            .dst_offset(vk::Offset3D {
                                x: rect.offset.x,
                                y: rect.offset.y,
                                z: 0,
                            })
                            .extent(vk::Extent3D {
                                width: rect.extent.width,
                                height: rect.extent.height,
                                depth: 1,
                            })],
                    );
                } else {
                    self.device.cmd_blit_image(
                        command,
                        texture.shared.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        atlas.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[vk::ImageBlit::default()
                            .src_subresource(layers)
                            .src_offsets([
                                vk::Offset3D::default(),
                                vk::Offset3D {
                                    x: source_size.w,
                                    y: source_size.h,
                                    z: 1,
                                },
                            ])
                            .dst_subresource(layers)
                            .dst_offsets([
                                vk::Offset3D {
                                    x: rect.offset.x,
                                    y: rect.offset.y,
                                    z: 0,
                                },
                                vk::Offset3D {
                                    x: rect.offset.x + rect.extent.width as i32,
                                    y: rect.offset.y + rect.extent.height as i32,
                                    z: 1,
                                },
                            ])],
                        vk::Filter::LINEAR,
                    );
                }
                self.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[source_barrier
                        .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                        .dst_access_mask(vk::AccessFlags::SHADER_READ)],
                );
            }
            self.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[atlas_barrier
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)],
            );
        }
        atlas.initialized = true;
    }

    fn pipeline(
        &self,
        fragment: &str,
        trace_through_transparent_windows: bool,
        ambient_occlusion: bool,
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
            ambient_occlusion,
        )?;
        let fragment_code = shader(
            fragment,
            naga::ShaderStage::Fragment,
            trace_through_transparent_windows,
            ambient_occlusion,
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
                self.environment_pipelines[usize::from(self.ambient_occlusion) * 2
                    + usize::from(self.trace_through_transparent_windows)],
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
                let mut cursor_position = close_hit.map_or([0.0; 4], |(_, position)| {
                    [
                        position.x + self.window_padding_px,
                        position.y + self.window_padding_px,
                        0.0,
                        0.0,
                    ]
                });
                cursor_position[2] = frame.texture_sample_phase as f32;
                let cursor_position_bytes =
                    std::slice::from_raw_parts(cursor_position.as_ptr().cast::<u8>(), 16);
                self.device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    64,
                    cursor_position_bytes,
                );
                eye[0] = geometry.logical_size.w as f32
                    + self.window_padding_px * 2.0
                    + self.max_border_width_px
                    + 2.0;
                eye[1] = geometry.logical_size.h as f32
                    + self.window_padding_px * 2.0
                    + self.max_border_width_px
                    + 2.0;
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
                self.draw_panel(
                    command,
                    projection
                        * expanded_window_model(
                            geometry,
                            self.window_padding_px,
                            self.max_border_width_px,
                        ),
                );
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
            for pipeline in self.environment_pipelines {
                self.device.destroy_pipeline(pipeline, None);
            }
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
    fn reflection_atlas_packs_native_sizes_without_overlap() {
        for sizes in [
            vec![],
            vec![vk::Extent2D {
                width: 1,
                height: 1,
            }],
            vec![
                vk::Extent2D {
                    width: 5,
                    height: 2,
                },
                vk::Extent2D {
                    width: 3,
                    height: 8,
                },
                vk::Extent2D {
                    width: 7,
                    height: 4,
                },
                vk::Extent2D {
                    width: 1,
                    height: 1,
                },
            ],
            vec![
                vk::Extent2D {
                    width: 8,
                    height: 8
                };
                4
            ],
        ] {
            let (extent, rects) = pack_reflection_atlas(&sizes, 16).unwrap();
            assert_eq!(extent.width, 16);
            assert_eq!(extent.height, 16);
            assert_eq!(rects.len(), sizes.len());
            let mut occupied = std::collections::HashSet::new();
            for (size, rect) in sizes.iter().zip(rects) {
                assert!(size == &rect.extent);
                for y in rect.offset.y as u32..rect.offset.y as u32 + size.height {
                    for x in rect.offset.x as u32..rect.offset.x as u32 + size.width {
                        assert!(x < extent.width && y < extent.height);
                        assert!(occupied.insert((x, y)));
                    }
                }
            }
        }
    }

    #[test]
    fn reflection_atlas_rejects_invalid_sizes_and_capacity_overflow() {
        for size in [
            vk::Extent2D {
                width: 0,
                height: 1,
            },
            vk::Extent2D {
                width: 1,
                height: 0,
            },
        ] {
            assert!(pack_reflection_atlas(&[size], 16).is_err());
        }
        assert!(
            pack_reflection_atlas(
                &[vk::Extent2D {
                    width: 1,
                    height: 1
                }; 257],
                16
            )
            .is_err()
        );
        assert!(pack_reflection_atlas(&[], 0).is_err());
    }

    #[test]
    fn atlas_only_marks_redrawn_windows_dirty_unless_layout_changes() {
        assert_eq!(atlas_dirty_indices(&[10, 20], &[10, 20], false, false), []);
        assert_eq!(atlas_dirty_indices(&[10, 20], &[10, 21], false, false), [1]);
        assert_eq!(
            atlas_dirty_indices(&[10, 20], &[10, 20], true, false),
            [0, 1]
        );
        assert_eq!(atlas_dirty_indices(&[], &[10, 20], false, true), [0, 1]);
    }

    #[test]
    fn fixed_atlas_downscales_all_windows_proportionally() {
        let sizes = [vk::Extent2D {
            width: 1920,
            height: 1080,
        }; 3];
        let (extent, rects) = pack_reflection_atlas(&sizes, 1024).unwrap();
        assert_eq!(extent.width, 1024);
        assert_eq!(extent.height, 1024);
        assert_eq!(rects.len(), 3);
        let mut occupied = std::collections::HashSet::new();
        for rect in &rects {
            assert!(rect.extent.width > 1 && rect.extent.width < 1920);
            assert!(rect.extent.height > 1 && rect.extent.height < 1080);
            assert!(
                (rect.extent.width as f64 / rect.extent.height as f64 - 1920.0 / 1080.0).abs()
                    < 0.01
            );
            assert!(rect.extent == rects[0].extent);
            for y in rect.offset.y as u32..rect.offset.y as u32 + rect.extent.height {
                for x in rect.offset.x as u32..rect.offset.x as u32 + rect.extent.width {
                    assert!(x < 1024 && y < 1024);
                    assert!(occupied.insert((x, y)));
                }
            }
        }
        let (_, minimum) = pack_reflection_atlas(
            &[vk::Extent2D {
                width: 100,
                height: 1,
            }; 16],
            4,
        )
        .unwrap();
        assert_eq!(minimum.len(), 16);
        assert!(
            minimum
                .iter()
                .all(|rect| rect.extent.width == 1 && rect.extent.height == 1)
        );
    }

    #[test]
    fn shaders_compile() {
        for (entry, stage) in [
            ("sky_vertex", naga::ShaderStage::Vertex),
            ("vertex", naga::ShaderStage::Vertex),
            ("window", naga::ShaderStage::Fragment),
            ("cursor", naga::ShaderStage::Fragment),
            ("environment", naga::ShaderStage::Fragment),
        ] {
            for ambient_occlusion in [false, true] {
                assert_eq!(
                    shader(entry, stage, false, ambient_occlusion).unwrap()[0],
                    0x0723_0203
                );
            }
            for trace_transparent in [false, true] {
                for ambient_occlusion in [false, true] {
                    assert_eq!(
                        shader(
                            "environment",
                            naga::ShaderStage::Fragment,
                            trace_transparent,
                            ambient_occlusion,
                        )
                        .unwrap()[0],
                        0x0723_0203
                    );
                }
            }
        }
        assert_eq!(
            shader("environment", naga::ShaderStage::Fragment, true, false).unwrap()[0],
            0x0723_0203
        );
    }

    #[test]
    fn skybox_mip_chain_reduces_dimensions_and_preserves_constant_color() {
        let base = (0..4 * 2)
            .flat_map(|_| {
                [
                    f16::from_f32(2.0),
                    f16::from_f32(1.0),
                    f16::from_f32(0.5),
                    f16::from_f32(1.0),
                ]
            })
            .collect();
        let (pixels, mips) = build_skybox_mips(4, 2, base);

        assert_eq!(
            mips.iter()
                .map(|mip| (mip.width, mip.height))
                .collect::<Vec<_>>(),
            [(4, 2), (2, 1), (1, 1)]
        );
        assert_eq!(pixels.len(), (4 * 2 + 2 + 1) * 4);
        for mip in mips {
            let offset = mip.buffer_offset as usize / std::mem::size_of::<f16>();
            for pixel in pixels[offset..offset + (mip.width * mip.height * 4) as usize].chunks(4) {
                assert_eq!(pixel[0].to_f32(), 2.0);
                assert_eq!(pixel[1].to_f32(), 1.0);
                assert_eq!(pixel[2].to_f32(), 0.5);
                assert_eq!(pixel[3].to_f32(), 1.0);
            }
        }
    }

    #[test]
    fn skybox_diffuse_integrates_only_the_upper_hemisphere() {
        let mut pixels = Vec::new();
        for y in 0..4 {
            let color = if y < 2 {
                [2.0, 1.0, 0.5]
            } else {
                [100.0, 100.0, 100.0]
            };
            for _ in 0..4 {
                pixels.extend([color[0], color[1], color[2], 1.0]);
            }
        }

        let rgba = bytemuck::cast_slice::<f32, Vec4>(&pixels);
        assert_eq!(rgba[0].to_array(), [2.0, 1.0, 0.5, 1.0]);
        let diffuse = integrate_skybox_diffuse(4, 4, rgba);
        assert!((diffuse[0] - 2.0).abs() < 1.0e-6, "{diffuse:?}");
        assert!((diffuse[1] - 1.0).abs() < 1.0e-6, "{diffuse:?}");
        assert!((diffuse[2] - 0.5).abs() < 1.0e-6, "{diffuse:?}");
    }

    #[test]
    fn skybox_diffuse_uses_lambertian_cosine_weighting() {
        let mut pixels = vec![0.0; 8 * 4 * 4];
        pixels[0] = 1.0;

        let diffuse = integrate_skybox_diffuse(4, 8, bytemuck::cast_slice::<f32, Vec4>(&pixels));
        let expected = (std::f32::consts::PI / 8.0).sin().powi(2) / 4.0;
        assert!((diffuse[0] - expected).abs() < 1.0e-6);
        assert_eq!(diffuse[1], 0.0);
        assert_eq!(diffuse[2], 0.0);
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
        assert!(
            (geometry.pose.center.x - left.x - (right.x - geometry.pose.center.x)).abs() < 1.0e-5
        );
        assert!((right.x - left.x - 2.26).abs() < 1.0e-5);
    }
}
