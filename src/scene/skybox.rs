use anyhow::{Context, Result, ensure};
use ash::vk;
use glam::{Mat4, Quat, Vec3, Vec4, Vec4Swizzles};
use half::f16;
use openxr as xr;
use rayon::prelude::*;
use std::{
    fs::{self, File},
    io::Read,
    path::PathBuf,
    thread::{self, JoinHandle},
};

use super::SceneRenderer;
use super::resources::memory_type;
use rand::{RngExt, SeedableRng, rngs::StdRng};

#[derive(Clone, Copy)]
struct SkyboxMip {
    width: u32,
    height: u32,
    buffer_offset: u64,
}

const SKYBOX_MAX_CHANNEL: f32 = 65_504.0;

fn sanitize_hdr_pixels(pixels: &mut [f32], mut random_state: u64) -> Result<()> {
    ensure!(
        pixels.len().is_multiple_of(4),
        "skybox pixel data must contain RGBA values"
    );
    let pixel_count = pixels.len() / 4;
    if random_state == 0 {
        random_state = 0x9e37_79b9_7f4a_7c15;
    }
    let mut random = StdRng::seed_from_u64(random_state);

    for index in 0..pixels.len() {
        if pixels[index].is_nan() {
            let pixel_index = index / 4;
            let channel = index % 4;
            let mut replacement = 0.0;
            if pixel_count > 1 {
                let start = random.random_range(0..pixel_count);
                for offset in 0..pixel_count {
                    let candidate_pixel = (start + offset) % pixel_count;
                    let candidate_index = candidate_pixel * 4 + channel;
                    if candidate_pixel != pixel_index && !pixels[candidate_index].is_nan() {
                        replacement = pixels[candidate_index].clamp(0.0, SKYBOX_MAX_CHANNEL);
                        break;
                    }
                }
            }
            pixels[index] = replacement;
        } else {
            pixels[index] = pixels[index].clamp(0.0, SKYBOX_MAX_CHANNEL);
        }
    }
    Ok(())
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
    pub(super) view: vk::ImageView,
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
        let mut pixels = pixels.into_raw();
        if pixels.iter().any(|value| value.is_nan()) {
            let mut random_state = [0; 8];
            File::open("/dev/urandom")
                .context("open system random source to repair skybox NaNs")?
                .read_exact(&mut random_state)
                .context("read system random source to repair skybox NaNs")?;
            sanitize_hdr_pixels(&mut pixels, u64::from_ne_bytes(random_state))?;
        } else {
            sanitize_hdr_pixels(&mut pixels, 1)?;
        }
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
            let skybox_format = vk::Format::R16G16B16A16_SFLOAT;
            let format_features = instance
                .get_physical_device_format_properties(physical_device, skybox_format)
                .optimal_tiling_features;
            ensure!(
                format_features.contains(
                    vk::FormatFeatureFlags::SAMPLED_IMAGE | vk::FormatFeatureFlags::TRANSFER_DST
                ),
                "GPU does not support sampled RGBA16F skybox textures"
            );
            skybox.image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(skybox_format)
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
                    .format(skybox_format)
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

pub(super) fn sky_matrix(view: &xr::View) -> Mat4 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn skybox_hdr_sanitization_replaces_nan_and_clamps_values() {
        let mut pixels = [
            f32::NAN,
            -2.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            3.0,
            4.0,
            5.0,
            6.0,
        ];
        sanitize_hdr_pixels(&mut pixels, 123).unwrap();

        assert_eq!(pixels[0], 3.0);
        assert_eq!(pixels[1], 0.0);
        assert_eq!(pixels[2], SKYBOX_MAX_CHANNEL);
        assert_eq!(pixels[3], 0.0);
        assert!(pixels.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn skybox_hdr_sanitization_handles_nan_only_images() {
        let mut pixels = [f32::NAN; 4];
        sanitize_hdr_pixels(&mut pixels, 123).unwrap();
        assert_eq!(pixels, [0.0; 4]);
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
}
