use anyhow::{Result, ensure};
use ash::vk;
use glam::Vec3;

use super::atlas::ReflectionAtlas;
use super::environment::{FloorUniform, WindowBufferHeader, WindowGpuData};
use super::resources::memory_type;
use super::{EnvironmentRenderer, FloorRenderer, RenderContext, SceneRenderer, WindowRenderer};
use crate::config::AppConfig;

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
            atlas: ReflectionAtlas::new(config.window.reflection_atlas_size.into()),
            window: WindowRenderer {
                trace_through_transparent_windows: config.floor.trace_through_transparent_windows,
                ambient_occlusion: config.floor.ambient_occlusion,
                window_padding_px: config.window.effective_padding_px(),
                max_border_width_px: config.window.max_border_width_px(),
            },
        };
        ReflectionAtlas::validate_size(&renderer.context, renderer.atlas.size)?;
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
