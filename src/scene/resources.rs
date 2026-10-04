use super::*;
use rand::{Rng, SeedableRng, rngs::StdRng};

#[derive(Clone, Copy)]
pub(super) struct SkyboxMip {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) buffer_offset: u64,
}

pub(super) const SKYBOX_MAX_CHANNEL: f32 = 65_504.0;

pub(super) fn sanitize_hdr_pixels(pixels: &mut [f32], mut random_state: u64) -> Result<()> {
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

pub(super) fn build_skybox_mips(
    width: u32,
    height: u32,
    mut pixels: Vec<f16>,
) -> (Vec<f16>, Vec<SkyboxMip>) {
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

pub(super) fn integrate_skybox_diffuse(width: u32, height: u32, pixels: &[Vec4]) -> Vec3 {
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
    pub(super) diffuse_irradiance: Vec3,
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
    pub(super) id: u64,
    pub shared: SharedImage,
    view: vk::ImageView,
    pool: vk::DescriptorPool,
    pub(super) descriptor: vk::DescriptorSet,
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
    pub(super) framebuffer: vk::Framebuffer,
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

pub(super) fn pack_reflection_atlas(
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

pub(super) fn atlas_dirty_indices(
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

pub(super) struct ReflectionAtlasImage {
    device: ash::Device,
    pub(super) image: vk::Image,
    memory: vk::DeviceMemory,
    pub(super) view: vk::ImageView,
    pub(super) extent: vk::Extent2D,
    pub(super) initialized: bool,
}

impl ReflectionAtlasImage {
    pub(super) fn new(renderer: &SceneRenderer, extent: vk::Extent2D) -> Result<Self> {
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

impl Drop for ReflectionAtlasImage {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}
