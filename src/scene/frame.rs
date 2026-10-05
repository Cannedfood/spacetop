use super::*;

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
        let (extent, rects) = pack_reflection_atlas(&sizes, self.atlas.size)?;
        let atlas_changed = self
            .atlas
            .texture
            .as_ref()
            .is_none_or(|atlas| atlas.extent != extent);
        let layout_changed = rects != self.atlas.rects;
        if atlas_changed {
            self.atlas.texture = Some(ReflectionAtlasImage::new(self, extent)?);
        }
        let panel_ids = frame
            .panels
            .iter()
            .map(|(texture, _)| texture.id)
            .collect::<Vec<_>>();
        self.atlas.dirty_indices = atlas_dirty_indices(
            &self.atlas.panel_ids,
            &panel_ids,
            layout_changed,
            atlas_changed,
        );
        self.atlas.rects = rects;
        self.atlas.panel_ids = panel_ids;
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

    pub unsafe fn copy_reflection_textures(
        &mut self,
        command: vk::CommandBuffer,
        frame: &SceneFrame<'_>,
    ) {
        let device = &self.context.device;
        if self.atlas.dirty_indices.is_empty()
            && self
                .atlas
                .texture
                .as_ref()
                .is_none_or(|atlas| atlas.initialized)
        {
            return;
        }
        let Some(atlas) = &mut self.atlas.texture else {
            return;
        };
        let range = image_range(vk::ImageAspectFlags::COLOR);
        let atlas_barrier = vk::ImageMemoryBarrier::default()
            .image(atlas.image)
            .subresource_range(range)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        unsafe {
            device.cmd_pipeline_barrier(
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
            for &index in &self.atlas.dirty_indices {
                let (texture, _) = &frame.panels[index];
                let rect = &self.atlas.rects[index];
                let source_barrier = vk::ImageMemoryBarrier::default()
                    .image(texture.shared.image)
                    .subresource_range(range)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                device.cmd_pipeline_barrier(
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
                    device.cmd_copy_image(
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
                    device.cmd_blit_image(
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
                device.cmd_pipeline_barrier(
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
            device.cmd_pipeline_barrier(
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
}
