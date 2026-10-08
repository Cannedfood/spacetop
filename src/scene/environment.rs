use anyhow::{Context, Result, ensure};
use ash::vk;
use glam::Vec3;

use super::atlas::ReflectionAtlas;
use super::resources::memory_type;
use super::{SceneFrame, SceneRenderer};
use crate::config::AppConfig;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(super) struct FloorUniform {
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
pub(super) struct WindowGpuData {
    center_width: [f32; 4],
    right_height: [f32; 4],
    up: [f32; 4],
    atlas_rect: [f32; 4],
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(super) struct WindowBufferHeader {
    count: u32,
    padding: [u32; 3],
}

impl FloorUniform {
    pub(super) fn from_config(
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
    pub fn update_config(&mut self, config: &AppConfig) -> Result<()> {
        let atlas_size = config.window.reflection_atlas_size.into();
        ReflectionAtlas::validate_size(&self.context, atlas_size)?;
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

impl SceneRenderer {
    pub fn prepare_frame(&mut self, frame: &SceneFrame<'_>) -> Result<()> {
        let skybox = frame.skybox.context("environment pass requires a skybox")?;
        if self.environment.dim != frame.environment_dim {
            let byte_offset =
                std::mem::offset_of!(FloorUniform, sampling) + 3 * std::mem::size_of::<f32>();
            let exposure = self.environment.background_exposure * (1.0 - frame.environment_dim);
            unsafe {
                let mapped = self.device.map_memory(
                    self.floor.memory,
                    0,
                    std::mem::size_of::<FloorUniform>() as u64,
                    vk::MemoryMapFlags::empty(),
                )?;
                std::ptr::copy_nonoverlapping(
                    (&exposure as *const f32).cast::<u8>(),
                    (mapped.cast::<u8>()).add(byte_offset),
                    std::mem::size_of::<f32>(),
                );
                self.device.unmap_memory(self.floor.memory);
            }
            self.environment.dim = frame.environment_dim;
        }
        let window_count =
            u32::try_from(frame.panels.len()).context("too many windows for Vulkan reflections")?;
        ensure!(
            window_count <= self.environment.max_windows,
            "window count {window_count} exceeds this GPU's reflected-window capacity {}",
            self.environment.max_windows
        );
        let atlas_changed = self.atlas.prepare(&self.context, frame.panels)?;
        let descriptor_count = 1;
        let descriptor_changed = descriptor_count > self.environment.descriptor_capacity;
        if descriptor_changed {
            unsafe {
                if self.environment.pool != vk::DescriptorPool::null() {
                    self.device
                        .destroy_descriptor_pool(self.environment.pool, None);
                    self.environment.pool = vk::DescriptorPool::null();
                    self.environment.descriptor = vk::DescriptorSet::null();
                }
                self.environment.pool = self.device.create_descriptor_pool(
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
                    .descriptor_pool(self.environment.pool)
                    .set_layouts(&layouts);
                self.environment.descriptor = self.device.allocate_descriptor_sets(&allocation)?[0];
                self.environment.descriptor_capacity = descriptor_count;
                self.environment.skybox_view = vk::ImageView::null();
            }
        }

        let required_buffer_size = std::mem::size_of::<WindowBufferHeader>() as u64
            + frame.panels.len() as u64 * std::mem::size_of::<WindowGpuData>() as u64;
        let mut buffer_changed = false;
        if required_buffer_size > self.environment.buffer_size {
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
                if self.environment.buffer != vk::Buffer::null() {
                    self.device.destroy_buffer(self.environment.buffer, None);
                    self.device.free_memory(self.environment.memory, None);
                }
            }
            self.environment.buffer = buffer;
            self.environment.memory = memory;
            self.environment.buffer_size = allocation_size;
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
                        let rect = self.atlas.rects[index];
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
            .texture
            .as_ref()
            .context("reflection atlas was not initialized")?
            .view;
        unsafe {
            let mapped = self.device.map_memory(
                self.environment.memory,
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
            self.device.unmap_memory(self.environment.memory);

            let skybox_changed = skybox.view != self.environment.skybox_view;
            if descriptor_changed {
                let filtering = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
                let skybox_filtering =
                    [vk::DescriptorImageInfo::default().sampler(self.sky_sampler)];
                self.device.update_descriptor_sets(
                    &[
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.environment.descriptor)
                            .dst_binding(1)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&filtering),
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.environment.descriptor)
                            .dst_binding(3)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&skybox_filtering),
                    ],
                    &[],
                );
            }
            if descriptor_changed || buffer_changed {
                let window_buffer = [vk::DescriptorBufferInfo::default()
                    .buffer(self.environment.buffer)
                    .range(required_buffer_size)];
                self.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.environment.descriptor)
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
                        .dst_set(self.environment.descriptor)
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&skybox_image)],
                    &[],
                );
                self.environment.skybox_view = skybox.view;
            }
            if descriptor_changed || atlas_changed || skybox_changed {
                let window_images = [vk::DescriptorImageInfo::default()
                    .image_view(atlas_view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                self.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.environment.descriptor)
                        .dst_binding(4)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&window_images)],
                    &[],
                );
            }
        }
        Ok(())
    }
}
