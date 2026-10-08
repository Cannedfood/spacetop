use anyhow::{Context, Result, ensure};
use ash::vk;

use super::resources::{PanelTextureId, image_range, image_view, memory_type};
use super::{PanelTexture, RenderContext};
use crate::panel::PanelGeometry;
use smithay::backend::allocator::Buffer;

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
    previous_ids: &[PanelTextureId],
    current_ids: &[PanelTextureId],
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
    image: vk::Image,
    memory: vk::DeviceMemory,
    pub(super) view: vk::ImageView,
    extent: vk::Extent2D,
    initialized: bool,
}

impl ReflectionAtlasImage {
    fn new(context: &RenderContext, extent: vk::Extent2D) -> Result<Self> {
        let device = &context.device;
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
                            &context.instance,
                            context.physical_device,
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

pub(super) struct ReflectionAtlas {
    pub(super) texture: Option<ReflectionAtlasImage>,
    pub(super) rects: Vec<vk::Rect2D>,
    panel_ids: Vec<PanelTextureId>,
    dirty_indices: Vec<usize>,
    pub(super) size: u32,
}

impl ReflectionAtlas {
    pub(super) fn new(size: u32) -> Self {
        Self {
            texture: None,
            rects: Vec::new(),
            panel_ids: Vec::new(),
            dirty_indices: Vec::new(),
            size,
        }
    }

    pub(super) fn prepare(
        &mut self,
        context: &RenderContext,
        panels: &[(&PanelTexture, PanelGeometry)],
    ) -> Result<bool> {
        let sizes = panels
            .iter()
            .map(|(texture, _)| {
                let size = texture.shared.dmabuf.size();
                vk::Extent2D {
                    width: size.w as u32,
                    height: size.h as u32,
                }
            })
            .collect::<Vec<_>>();
        let (extent, rects) = pack_reflection_atlas(&sizes, self.size)?;
        let atlas_changed = self
            .texture
            .as_ref()
            .is_none_or(|atlas| atlas.extent != extent);
        let layout_changed = rects != self.rects;
        if atlas_changed {
            self.texture = Some(ReflectionAtlasImage::new(context, extent)?);
        }
        let panel_ids = panels
            .iter()
            .map(|(texture, _)| texture.id)
            .collect::<Vec<_>>();
        self.dirty_indices =
            atlas_dirty_indices(&self.panel_ids, &panel_ids, layout_changed, atlas_changed);
        self.rects = rects;
        self.panel_ids = panel_ids;
        Ok(atlas_changed)
    }

    pub(super) fn validate_size(context: &RenderContext, size: u32) -> Result<()> {
        let limit = unsafe {
            context
                .instance
                .get_physical_device_properties(context.physical_device)
        }
        .limits
        .max_image_dimension2_d;
        ensure!(
            size <= limit,
            "reflection atlas size {size} exceeds this GPU's maximum 2D image dimension {limit}"
        );
        let properties = unsafe {
            context.instance.get_physical_device_format_properties(
                context.physical_device,
                vk::Format::R8G8B8A8_SRGB,
            )
        };
        let mut modifiers = vk::DrmFormatModifierPropertiesListEXT::default();
        unsafe {
            context.instance.get_physical_device_format_properties2(
                context.physical_device,
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
            context.instance.get_physical_device_format_properties2(
                context.physical_device,
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
}

impl ReflectionAtlas {
    pub(super) unsafe fn copy_textures(
        &mut self,
        device: &ash::Device,
        command: vk::CommandBuffer,
        panels: &[(&PanelTexture, PanelGeometry)],
    ) {
        if self.dirty_indices.is_empty()
            && self.texture.as_ref().is_none_or(|atlas| atlas.initialized)
        {
            return;
        }
        let Some(atlas) = &mut self.texture else {
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
            for &index in &self.dirty_indices {
                let (texture, _) = &panels[index];
                let rect = &self.rects[index];
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
        let first = PanelTextureId(10);
        let second = PanelTextureId(20);
        let changed = PanelTextureId(21);
        assert_eq!(
            atlas_dirty_indices(&[first, second], &[first, second], false, false),
            []
        );
        assert_eq!(
            atlas_dirty_indices(&[first, second], &[first, changed], false, false),
            [1]
        );
        assert_eq!(
            atlas_dirty_indices(&[first, second], &[first, second], true, false),
            [0, 1]
        );
        assert_eq!(
            atlas_dirty_indices(&[], &[first, second], false, true),
            [0, 1]
        );
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
}
