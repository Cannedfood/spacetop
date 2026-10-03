use super::*;
use crate::{
    panel::{PanelGeometry, PanelPose},
    scene::{
        FALLBACK_FLOOR_Y, PanelTexture, RenderTarget, SceneFrame, SceneRenderer, SkyboxTexture,
    },
};

struct SceneReadback<'a> {
    renderer: &'a SceneRenderer,
    view: &'a openxr::View,
    skybox: Option<&'a mut SkyboxTexture>,
    panels: &'a [(&'a PanelTexture, PanelGeometry)],
    cursor: Option<PanelPose>,
    floor_y: f32,
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
        mut scene: Option<SceneReadback<'_>>,
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
            let target = if let Some(scene) = scene.as_mut() {
                let target = RenderTarget::new(
                    scene.renderer,
                    destination,
                    vk::Extent2D {
                        width: size.w as u32,
                        height: size.h as u32,
                    },
                )?;
                if let Some(skybox) = scene
                    .skybox
                    .as_deref()
                    .filter(|skybox| skybox.needs_upload())
                {
                    skybox.upload(command);
                }
                for (texture, _) in scene.panels {
                    texture.ownership(command, self.queue_family, true);
                }
                scene.renderer.draw(
                    command,
                    &target,
                    scene.view,
                    &SceneFrame {
                        skybox: scene.skybox.as_deref(),
                        panels: scene.panels,
                        cursor: scene.cursor,
                        floor_y: scene.floor_y,
                    },
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
            if let Some(scene) = scene.as_mut()
                && let Some(skybox) = scene.skybox.as_deref_mut()
                && skybox.needs_upload()
            {
                skybox.upload_complete();
            }
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
    let scene = SceneRenderer::new(
        &vulkan.device,
        &vulkan.instance,
        vulkan.physical_device,
        vk::Format::R8G8B8A8_SRGB,
        &crate::config::AppConfig::default(),
    )?;
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
    let second_background = make_texture(false)?;
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
            skybox: None,
            panels: &panels,
            cursor: Some(cursor),
            floor_y: FALLBACK_FLOOR_Y,
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
            skybox: None,
            panels: &panels,
            cursor: None,
            floor_y: FALLBACK_FLOOR_Y,
        }),
    )?;
    assert_eq!(pixel(&moved, 380, 200), [0, 0, 0, 255]);
    assert_eq!(pixel(&moved, 200, 200), [255, 0, 0, 255]);
    view.pose = openxr::Posef::IDENTITY;
    let emitter = PanelGeometry {
        pose: PanelPose {
            center: glam::Vec3::new(0.0, 0.0, -3.0),
            ..near.pose
        },
        ..near
    };
    let floor_pixels = |panels: &[(&PanelTexture, PanelGeometry)]| {
        vulkan.readback_image(
            None,
            (512, 512).into(),
            None,
            Some(SceneReadback {
                renderer: &scene,
                view: &view,
                skybox: None,
                panels,
                cursor: None,
                floor_y: FALLBACK_FLOOR_Y,
            }),
        )
    };
    let empty = floor_pixels(&[])?;
    let empty_floor_pixel = pixel(&empty, 256, 450);
    assert_eq!(empty_floor_pixel, [0, 0, 0, 255]);
    let lit = floor_pixels(&[(&background, emitter)])?;
    let mut brighter_sky_config = crate::config::AppConfig::default();
    brighter_sky_config.background.brightness_stops = 2.0;
    scene.update_config(&brighter_sky_config)?;
    let unchanged_reflection = floor_pixels(&[(&background, emitter)])?;
    assert_eq!(
        lit, unchanged_reflection,
        "skybox brightness must not alter window reflections"
    );
    scene.update_config(&crate::config::AppConfig::default())?;
    let repeated = floor_pixels(&[(&background, emitter)])?;
    assert_eq!(lit, repeated);
    let mut shifted_view = view;
    shifted_view.fov.angle_left = (-1.0_f32 - 16.0 / 256.0).atan();
    shifted_view.fov.angle_right = (1.0_f32 - 16.0 / 256.0).atan();
    let shifted = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &scene,
            view: &shifted_view,
            skybox: None,
            panels: &[(&background, emitter)],
            cursor: None,
            floor_y: FALLBACK_FLOOR_Y,
        }),
    )?;
    let mut anchored_pixels = 0;
    for vertical in 430..470 {
        for horizontal in 240..272 {
            if pixel(&lit, horizontal, vertical) == pixel(&shifted, horizontal + 16, vertical) {
                anchored_pixels += 1;
            }
        }
    }
    assert!(
        anchored_pixels >= 1250,
        "reflection noise must follow floor positions when their screen coordinates change"
    );
    let mut isolated_variations = 0;
    for vertical in 430..470 {
        for horizontal in 240..272 {
            let left = i32::from(pixel(&lit, horizontal - 1, vertical)[0]);
            let middle = i32::from(pixel(&lit, horizontal, vertical)[0]);
            let right = i32::from(pixel(&lit, horizontal + 1, vertical)[0]);
            if (middle > left && middle > right) || (middle < left && middle < right) {
                isolated_variations += 1;
            }
        }
    }
    assert!(
        isolated_variations > 100,
        "GGX samples must vary per pixel, not form coherent repeated reflections"
    );
    let doubled = floor_pixels(&[(&background, emitter), (&second_background, emitter)])?;
    let colored = floor_pixels(&[(&foreground, emitter)])?;
    let backwards = floor_pixels(&[(
        &background,
        PanelGeometry {
            pose: PanelPose {
                yaw: std::f32::consts::PI,
                ..emitter.pose
            },
            ..emitter
        },
    )])?;
    let decode = |channel: u8| {
        let encoded = f32::from(channel) / 255.0;
        if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        }
    };
    let floor_pixel = pixel(&lit, 256, 450);
    assert!(floor_pixel[0] > 0);
    assert!(floor_pixel[0] > empty_floor_pixel[0]);
    assert_eq!(floor_pixel[0], floor_pixel[1]);
    assert_eq!(floor_pixel[1], floor_pixel[2]);
    assert_eq!(floor_pixel[3], 255);
    assert_eq!(pixel(&backwards, 256, 450), empty_floor_pixel);
    let twice = pixel(&doubled, 256, 450);
    let quantization =
        |channel: u8| decode(channel.saturating_add(1)) - decode(channel.saturating_sub(1));
    assert!(
        (decode(twice[0]) - (2.0 * decode(floor_pixel[0]) - decode(empty_floor_pixel[0]))).abs()
            <= quantization(twice[0]) + 2.0 * quantization(floor_pixel[0])
    );
    assert_eq!(twice[3], 255);
    let colored_pixel = pixel(&colored, 256, 450);
    assert!(colored_pixel[..3].iter().any(|channel| *channel > 0));
    assert!(
        colored_pixel[..3]
            .iter()
            .zip(&floor_pixel[..3])
            .all(|(colored, white)| colored <= white)
    );
    assert_ne!(colored_pixel, floor_pixel);
    let lowered = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &scene,
            view: &view,
            skybox: None,
            panels: &[(&background, emitter)],
            cursor: None,
            floor_y: -2.6,
        }),
    )?;
    assert_eq!(pixel(&lowered, 256, 450), empty_floor_pixel);
    let mut stage_view = view;
    stage_view.pose.position.y = -FALLBACK_FLOOR_Y;
    let stage_emitter = PanelGeometry {
        pose: PanelPose {
            center: emitter.pose.center - glam::Vec3::Y * FALLBACK_FLOOR_Y,
            ..emitter.pose
        },
        ..emitter
    };
    let calibrated = vulkan.readback_image(
        None,
        (512, 512).into(),
        None,
        Some(SceneReadback {
            renderer: &scene,
            view: &stage_view,
            skybox: None,
            panels: &[(&background, stage_emitter)],
            cursor: None,
            floor_y: 0.0,
        }),
    )?;
    assert_eq!(pixel(&calibrated, 256, 450), floor_pixel);
    assert_eq!(pixel(&calibrated, 256, 256), pixel(&lit, 256, 256));
    Ok(())
}

#[test]
#[ignore = "requires Vulkan DMA-BUF support and a configured EXR skybox"]
fn vulkan_scene_renders_equirectangular_skybox() -> Result<()> {
    let vulkan = Vulkan::new()?;
    let renderer = SceneRenderer::new(
        &vulkan.device,
        &vulkan.instance,
        vulkan.physical_device,
        vk::Format::R8G8B8A8_SRGB,
        &crate::config::AppConfig::default(),
    )?;
    let mut skybox = SkyboxTexture::new(
        &renderer,
        &vulkan.instance,
        vulkan.physical_device,
        "random",
    )?;
    let view = openxr::View {
        pose: openxr::Posef::IDENTITY,
        fov: openxr::Fovf {
            angle_left: -std::f32::consts::FRAC_PI_4,
            angle_right: std::f32::consts::FRAC_PI_4,
            angle_up: std::f32::consts::FRAC_PI_4,
            angle_down: -std::f32::consts::FRAC_PI_4,
        },
    };
    let pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: FALLBACK_FLOOR_Y,
        }),
    )?;
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255)
    );
    let colors = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect::<std::collections::HashSet<_>>();
    assert!(colors.len() > 8, "skybox should vary across the view");
    let ground_colors = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(index, _)| index / 128 >= 56)
        .map(|(_, pixel)| [pixel[0], pixel[1], pixel[2]])
        .collect::<std::collections::HashSet<_>>();
    assert!(
        ground_colors.len() > 1,
        "transparent floor should reveal variations in the skybox"
    );

    let brightness_sum = |pixels: &[u8]| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| u64::from(pixel[0]) + u64::from(pixel[1]) + u64::from(pixel[2]))
            .sum::<u64>()
    };
    let base_brightness = brightness_sum(&pixels);
    let mut brighter_config = crate::config::AppConfig::default();
    brighter_config.background.brightness_stops = 1.0;
    renderer.update_config(&brighter_config)?;
    let brighter_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: FALLBACK_FLOOR_Y,
        }),
    )?;
    assert!(
        brightness_sum(&brighter_pixels) > base_brightness,
        "increasing skybox exposure by one stop should brighten the rendered image"
    );
    let mut rotated_config = crate::config::AppConfig::default();
    rotated_config.background.rotation_degrees = 90.0;
    renderer.update_config(&rotated_config)?;
    let rotated_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: FALLBACK_FLOOR_Y,
        }),
    )?;
    assert_ne!(
        pixels, rotated_pixels,
        "skybox rotation should change the view"
    );
    let mut reflective_floor_config = crate::config::AppConfig::default();
    reflective_floor_config.floor.albedo = [0.0, 0.0, 0.0, 1.0];
    reflective_floor_config.floor.reflectance = 1.0;
    reflective_floor_config.floor.roughness = 0.0;
    renderer.update_config(&reflective_floor_config)?;
    let reflected_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: FALLBACK_FLOOR_Y,
        }),
    )?;
    let sky_only_pixels = vulkan.readback_image(
        None,
        (128, 64).into(),
        None,
        Some(SceneReadback {
            renderer: &renderer,
            view: &view,
            skybox: Some(&mut skybox),
            panels: &[],
            cursor: None,
            floor_y: f32::NAN,
        }),
    )?;
    let ground_start = 56 * 128 * 4;
    assert_ne!(
        &reflected_pixels[ground_start..],
        &sky_only_pixels[ground_start..],
        "ground should show the skybox reflected across its surface"
    );
    Ok(())
}

#[test]
#[ignore = "requires Vulkan DMA-BUF support and a configured EXR skybox"]
fn vulkan_floor_fresnel_dims_albedo_at_grazing_angles() -> Result<()> {
    let vulkan = Vulkan::new()?;
    let mut config = crate::config::AppConfig::default();
    config.floor.albedo = [0.2, 0.3, 0.4, 0.6];
    config.floor.reflectance = 0.12;
    let floor_config = &config.floor;
    let renderer = SceneRenderer::new(
        &vulkan.device,
        &vulkan.instance,
        vulkan.physical_device,
        vk::Format::R8G8B8A8_SRGB,
        &crate::config::AppConfig::default(),
    )?;
    renderer.update_config(&config)?;
    let skybox_directory = tempfile::tempdir()?;
    let skybox_path = skybox_directory.path().join("black.exr");
    image::Rgb32FImage::from_pixel(8, 4, image::Rgb([0.0, 0.0, 0.0])).save(&skybox_path)?;
    let skybox_path = skybox_path.to_string_lossy().into_owned();
    let mut skybox = SkyboxTexture::new(
        &renderer,
        &vulkan.instance,
        vulkan.physical_device,
        &skybox_path,
    )?;
    let view_at = |target: glam::Vec3, eye: glam::Vec3| {
        let orientation =
            glam::Quat::from_rotation_arc(glam::Vec3::NEG_Z, (target - eye).normalize());
        let mut view = openxr::View {
            pose: openxr::Posef::IDENTITY,
            fov: openxr::Fovf {
                angle_left: -std::f32::consts::FRAC_PI_4,
                angle_right: std::f32::consts::FRAC_PI_4,
                angle_up: std::f32::consts::FRAC_PI_4,
                angle_down: -std::f32::consts::FRAC_PI_4,
            },
        };
        view.pose.position.x = eye.x;
        view.pose.position.y = eye.y;
        view.pose.position.z = eye.z;
        view.pose.orientation.x = orientation.x;
        view.pose.orientation.y = orientation.y;
        view.pose.orientation.z = orientation.z;
        view.pose.orientation.w = orientation.w;
        view
    };
    let mut render = |view: &openxr::View, floor_y: f32| {
        vulkan.readback_image(
            None,
            (128, 128).into(),
            None,
            Some(SceneReadback {
                renderer: &renderer,
                view,
                skybox: Some(&mut skybox),
                panels: &[],
                cursor: None,
                floor_y,
            }),
        )
    };
    let head_target = glam::Vec3::new(0.0, FALLBACK_FLOOR_Y, -2.0);
    let head_view = view_at(head_target, glam::Vec3::new(0.0, 0.0, -2.0));
    let grazing_view = view_at(head_target, glam::Vec3::new(5.0, 0.0, -2.0));
    let head_on = render(&head_view, FALLBACK_FLOOR_Y)?;
    let head_sky = render(&head_view, f32::NAN)?;
    let grazing = render(&grazing_view, FALLBACK_FLOOR_Y)?;
    let grazing_sky = render(&grazing_view, f32::NAN)?;
    let center = (64 * 128 + 64) * 4;
    let decode = |channel: u8| {
        let encoded = f32::from(channel) / 255.0;
        if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        }
    };
    let recovered_albedo = |floor: &[u8], sky: &[u8]| {
        (decode(floor[center]) - (1.0 - floor_config.albedo[3]) * decode(sky[center]))
            / floor_config.albedo[3]
    };
    let head_albedo = recovered_albedo(&head_on, &head_sky);
    let expected_head_albedo = floor_config.albedo[0] * (1.0 - floor_config.reflectance);
    assert!(
        (head_albedo - expected_head_albedo).abs() < 0.02,
        "runtime floor settings should reach the shader: expected={expected_head_albedo}, actual={head_albedo}"
    );
    assert!(
        head_albedo > recovered_albedo(&grazing, &grazing_sky),
        "floor albedo should dim at grazing angles: head-on={}, grazing={}",
        head_albedo,
        recovered_albedo(&grazing, &grazing_sky)
    );
    let distant_target = glam::Vec3::new(0.0, FALLBACK_FLOOR_Y, -45.0);
    let distant_view = view_at(distant_target, glam::Vec3::ZERO);
    let distant_floor = render(&distant_view, FALLBACK_FLOOR_Y)?;
    let distant_sky = render(&distant_view, f32::NAN)?;
    assert_ne!(
        &distant_floor[center..center + 3],
        &distant_sky[center..center + 3],
        "ground shading should extend beyond the old 60m floor quad"
    );
    Ok(())
}
