use anyhow::{Context, Result};
use ash::vk;
use std::sync::atomic::{AtomicU64, Ordering};

use super::SceneRenderer;
use crate::gpu::SharedImage;

static NEXT_PANEL_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct PanelTextureId(pub(super) u64);

pub(super) fn image_range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(aspect)
        .level_count(1)
        .layer_count(1)
}

pub(super) fn memory_type(
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

pub(super) fn image_view(
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
    pub(super) id: PanelTextureId,
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
                break PanelTextureId(current);
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
