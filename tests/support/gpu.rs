use super::*;
use crate::{
    panel::{PanelGeometry, PanelPose},
    scene::{PanelTexture, RenderTarget, SceneRenderer},
};

struct SceneReadback<'a> {
    renderer: &'a SceneRenderer,
    view: &'a openxr::View,
    panels: &'a [(&'a PanelTexture, PanelGeometry)],
    cursor: Option<PanelPose>,
}

#[test]
fn cursor_rectangles_are_clipped_and_do_not_overlap() {
    for size in [(1, 1), (5, 3), (512, 512)] {
        for center in [(0, 0), (size.0 - 1, size.1 - 1), (size.0 / 2, size.1 / 2)] {
            let rectangles = cursor_rectangles(size.into(), center);
            let mut pixels = std::collections::HashSet::new();
            for rectangle in rectangles {
                assert!(rectangle.size.w * rectangle.size.h <= 21);
                for y in rectangle.loc.y..rectangle.loc.y + rectangle.size.h {
                    for x in rectangle.loc.x..rectangle.loc.x + rectangle.size.w {
                        assert!(x >= 0 && x < size.0 && y >= 0 && y < size.1);
                        assert!(pixels.insert((x, y)), "cursor copies must not overlap");
                    }
                }
            }
            assert!(pixels.contains(&center));
        }
    }
    assert!(cursor_rectangles((0, 0).into(), (0, 0)).is_empty());
}

pub struct Vulkan {
    _entry: ash::Entry,
    pub instance: ash::Instance,
    pub device: ash::Device,
    pub physical_device: vk::PhysicalDevice,
    pub render_node: PathBuf,
    queue_family: u32,
}

impl Vulkan {
    pub fn new() -> Result<Self> {
        let entry = unsafe { ash::Entry::load() }?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance = unsafe {
            entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
        }?;
        let selection = (|| -> Result<_> {
            for physical_device in unsafe { instance.enumerate_physical_devices() }? {
                if !sharing_supported(&instance, physical_device, vk::API_VERSION_1_1)? {
                    continue;
                }
                let Some(render_node) = render_node(&instance, physical_device) else {
                    continue;
                };
                if let Some(requested) = std::env::var_os("SPACETOP_GPU_TEST_NODE")
                    && Path::new(&requested) != render_node
                {
                    continue;
                }
                let queue_family = unsafe {
                    instance.get_physical_device_queue_family_properties(physical_device)
                }
                .iter()
                .position(|properties| properties.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .context("GPU lacks a graphics queue")? as u32;
                let priorities = [1.0];
                let queues = [vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(queue_family)
                    .queue_priorities(&priorities)];
                let extensions = SHARING_EXTENSIONS.map(|name| name.as_ptr());
                let info = vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queues)
                    .enabled_extension_names(&extensions);
                let device = unsafe { instance.create_device(physical_device, &info, None) }?;
                return Ok((physical_device, render_node, queue_family, device));
            }
            anyhow::bail!("no Vulkan GPU supports DMA-BUF sharing");
        })();
        match selection {
            Ok((physical_device, render_node, queue_family, device)) => {
                eprintln!("Testing GPU sharing on {}", render_node.display());
                Ok(Self {
                    _entry: entry,
                    instance,
                    device,
                    physical_device,
                    render_node,
                    queue_family,
                })
            }
            Err(error) => {
                unsafe {
                    instance.destroy_instance(None);
                }
                Err(error)
            }
        }
    }

    pub fn readback(&self, shared: &SharedImage, cursor: Option<(i32, i32)>) -> Result<Vec<u8>> {
        let size = shared.dmabuf.size();
        self.readback_image(Some(shared), size, cursor, None)
    }

    pub fn readback_cursor(&self) -> Result<Vec<u8>> {
        self.readback_image(None, (21, 21).into(), Some((10, 10)), None)
    }

    fn readback_image(
        &self,
        shared: Option<&SharedImage>,
        size: Size<i32, smithay::utils::Buffer>,
        cursor: Option<(i32, i32)>,
        scene: Option<SceneReadback<'_>>,
    ) -> Result<Vec<u8>> {
        let byte_len = size.w as u64 * size.h as u64 * 4;
        unsafe {
            let buffer = self.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(byte_len)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )?;
            let requirements = self.device.get_buffer_memory_requirements(buffer);
            let properties = self
                .instance
                .get_physical_device_memory_properties(self.physical_device);
            let memory_type = (0..properties.memory_type_count)
                .find(|index| {
                    requirements.memory_type_bits & (1 << index) != 0
                        && properties.memory_types[*index as usize]
                            .property_flags
                            .contains(
                                vk::MemoryPropertyFlags::HOST_VISIBLE
                                    | vk::MemoryPropertyFlags::HOST_COHERENT,
                            )
                })
                .context("no coherent readback memory")?;
            let memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type),
                None,
            )?;
            self.device.bind_buffer_memory(buffer, memory, 0)?;
            let image_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_SRGB)
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .extent(vk::Extent3D {
                    width: size.w as u32,
                    height: size.h as u32,
                    depth: 1,
                })
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(
                    vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::TRANSFER_DST
                        | vk::ImageUsageFlags::COLOR_ATTACHMENT,
                );
            let destination = self.device.create_image(&image_info, None)?;
            let image_requirements = self.device.get_image_memory_requirements(destination);
            let image_memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(image_requirements.size)
                    .memory_type_index(image_requirements.memory_type_bits.trailing_zeros()),
                None,
            )?;
            self.device
                .bind_image_memory(destination, image_memory, 0)?;
            let cursor_buffer = if cursor.is_some() {
                Some(GpuCursor::new(
                    &self.instance,
                    &self.device,
                    self.physical_device,
                )?)
            } else {
                None
            };
            let pool = self.device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(self.queue_family),
                None,
            )?;
            let command = self.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            self.device
                .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let barrier = vk::ImageMemoryBarrier::default()
                .image(destination)
                .subresource_range(range)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            self.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            if let Some(shared) = shared {
                shared.copy_to(&self.device, command, destination, self.queue_family);
            }
            if let (Some(buffer), Some(center)) = (cursor_buffer.as_ref(), cursor) {
                if shared.is_some() {
                    buffer.draw(command, destination, size, center);
                } else {
                    buffer.draw_quad(command, destination);
                }
            }
            let target = if let Some(scene) = scene {
                let target = RenderTarget::new(
                    scene.renderer,
                    destination,
                    vk::Extent2D {
                        width: size.w as u32,
                        height: size.h as u32,
                    },
                )?;
                for (texture, _) in scene.panels {
                    texture.ownership(command, self.queue_family, true);
                }
                scene.renderer.draw(
                    command,
                    &target,
                    scene.view,
                    scene.panels.iter().copied(),
                    scene.cursor,
                );
                for (texture, _) in scene.panels {
                    texture.ownership(command, self.queue_family, false);
                }
                Some(target)
            } else {
                None
            };
            let barrier = vk::ImageMemoryBarrier::default()
                .image(destination)
                .subresource_range(range)
                .old_layout(if target.is_some() {
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                } else {
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL
                })
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .src_access_mask(
                    vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                )
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
            self.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: size.w as u32,
                    height: size.h as u32,
                    depth: 1,
                });
            self.device.cmd_copy_image_to_buffer(
                command,
                destination,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            let host = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ);
            self.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[host],
                &[],
                &[],
            );
            self.device.end_command_buffer(command)?;
            let queue = self.device.get_device_queue(self.queue_family, 0);
            self.device.queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&[command])],
                vk::Fence::null(),
            )?;
            self.device.queue_wait_idle(queue)?;
            let mapped =
                self.device
                    .map_memory(memory, 0, byte_len, vk::MemoryMapFlags::empty())?;
            let pixels =
                std::slice::from_raw_parts(mapped.cast::<u8>(), byte_len as usize).to_vec();
            self.device.unmap_memory(memory);
            drop(cursor_buffer);
            drop(target);
            self.device.destroy_image(destination, None);
            self.device.free_memory(image_memory, None);
            self.device.destroy_command_pool(pool, None);
            self.device.destroy_buffer(buffer, None);
            self.device.free_memory(memory, None);
            Ok(pixels)
        }
    }
}

impl Drop for Vulkan {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[test]
#[ignore = "requires a Vulkan/GLES GPU with DMA-BUF sharing"]
fn vulkan_scene_renders_sampled_panels_and_cursor() -> Result<()> {
    let vulkan = Vulkan::new()?;
    let scene = SceneRenderer::new(&vulkan.device, vk::Format::R8G8B8A8_SRGB)?;
    let mut producer = GpuRenderer::new(&vulkan.render_node)?;
    let mut make_texture = |quadrants: bool| -> Result<PanelTexture> {
        let buffer =
            producer
                .allocator
                .create_buffer(32, 32, Fourcc::Abgr8888, &[Modifier::Linear])?;
        let mut dmabuf = buffer.export()?;
        {
            let mut framebuffer = producer.renderer.bind(&mut dmabuf)?;
            let mut frame =
                producer
                    .renderer
                    .render(&mut framebuffer, (32, 32).into(), Transform::Normal)?;
            let full = [smithay::utils::Rectangle::from_size((32, 32).into())];
            frame.clear(
                smithay::backend::renderer::Color32F::new(1.0, 1.0, 1.0, 1.0),
                &full,
            )?;
            if quadrants {
                for (location, color) in [
                    ((0, 0), [1.0, 0.0, 0.0, 1.0]),
                    ((16, 0), [0.0, 1.0, 0.0, 1.0]),
                    ((0, 16), [0.0, 0.0, 1.0, 1.0]),
                    ((16, 16), [0.0, 0.0, 0.0, 0.0]),
                ] {
                    frame.clear(
                        color.into(),
                        &[smithay::utils::Rectangle::new(
                            location.into(),
                            (16, 16).into(),
                        )],
                    )?;
                }
            }
            frame.finish()?.wait()?;
        }
        PanelTexture::new(
            &scene,
            SharedImage::import(
                &vulkan.instance,
                &vulkan.device,
                vulkan.physical_device,
                dmabuf,
            )?,
        )
    };
    let foreground = make_texture(true)?;
    let background = make_texture(false)?;
    let near = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -1.0),
            yaw: 0.0,
            pitch: 0.0,
            width_m: 1.0,
        },
        logical_size: (32, 32).into(),
    };
    let far = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -2.0),
            width_m: 2.0,
            ..near.pose
        },
        ..near
    };
    let panels = [(&foreground, near), (&background, far)];
    let mut view = openxr::View {
        pose: openxr::Posef::IDENTITY,
        fov: openxr::Fovf {
            angle_left: -std::f32::consts::FRAC_PI_4,
            angle_right: std::f32::consts::FRAC_PI_4,
            angle_up: std::f32::consts::FRAC_PI_4,
            angle_down: -std::f32::consts::FRAC_PI_4,
        },
    };
    let cursor = PanelPose {
        width_m: 0.021,
        ..near.pose
    };
    let pixels = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &scene,
            view: &view,
            panels: &panels,
            cursor: Some(cursor),
        }),
    )?;
    let pixel = |pixels: &[u8], horizontal: usize, vertical: usize| {
        pixels[(vertical * 512 + horizontal) * 4..][..4].to_vec()
    };
    assert_eq!(pixel(&pixels, 200, 200), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 300, 200), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, 200, 300), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, 300, 300), [255, 255, 255, 255]);
    assert_eq!(pixel(&pixels, 30, 30), [0, 0, 0, 255]);
    let cross = pixel(&pixels, 255, 255);
    assert_eq!(cross[0], 255);
    assert!(cross[1].abs_diff(245) <= 1);
    assert_eq!(&cross[2..], [0, 255]);
    assert_eq!(pixel(&pixels, 380, 200), [0, 255, 0, 255]);
    view.pose.position.x = 0.1;
    let moved = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &scene,
            view: &view,
            panels: &panels,
            cursor: None,
        }),
    )?;
    assert_eq!(pixel(&moved, 380, 200), [0, 0, 0, 255]);
    assert_eq!(pixel(&moved, 200, 200), [255, 0, 0, 255]);
    Ok(())
}
