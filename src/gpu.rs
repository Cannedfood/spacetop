use std::{
    ffi::CStr,
    fs::OpenOptions,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use ash::vk;
use smithay::{
    backend::{
        allocator::{
            Allocator, Buffer, Fourcc, Modifier,
            dmabuf::{AsDmabuf, Dmabuf},
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        egl::{EGLContext, EGLDisplay},
        renderer::{
            Bind, Frame, ImportDma, Renderer,
            element::{
                Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
            utils::draw_render_elements,
        },
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Physical, Size, Transform},
};

pub struct GpuRenderer {
    renderer: GlesRenderer,
    allocator: GbmAllocator<Arc<std::fs::File>>,
}

pub const SHARING_EXTENSIONS: [&CStr; 6] = [
    ash::khr::external_memory_fd::NAME,
    ash::ext::external_memory_dma_buf::NAME,
    ash::ext::image_drm_format_modifier::NAME,
    ash::ext::physical_device_drm::NAME,
    ash::khr::image_format_list::NAME,
    ash::ext::queue_family_foreign::NAME,
];

pub fn sharing_supported(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    api_version: u32,
) -> Result<bool> {
    if api_version < vk::API_VERSION_1_1 {
        return Ok(false);
    }
    if unsafe { instance.get_physical_device_properties(physical_device) }.api_version
        < vk::API_VERSION_1_1
    {
        return Ok(false);
    }
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical_device) }?;
    Ok(SHARING_EXTENSIONS.iter().all(|required| {
        extensions.iter().any(
            |extension| unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) } == *required,
        )
    }))
}

pub fn render_node(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Option<PathBuf> {
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
    unsafe {
        instance.get_physical_device_properties2(physical_device, &mut properties);
    }
    (drm.has_render != 0 && drm.render_major == 226)
        .then(|| PathBuf::from(format!("/dev/dri/renderD{}", drm.render_minor)))
}

pub struct GpuCursor {
    device: ash::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

fn cursor_rectangles(
    size: Size<i32, smithay::utils::Buffer>,
    center: (i32, i32),
) -> Vec<smithay::utils::Rectangle<i32, smithay::utils::Buffer>> {
    if size.w <= 0 || size.h <= 0 {
        return Vec::new();
    }
    let center_x = center.0.clamp(0, size.w - 1);
    let center_y = center.1.clamp(0, size.h - 1);
    let left = (center_x - 10).max(0);
    let right = (center_x + 11).min(size.w);
    let top = (center_y - 10).max(0);
    let bottom = (center_y + 11).min(size.h);
    let mut rectangles = vec![smithay::utils::Rectangle::new(
        (left, center_y).into(),
        (right - left, 1).into(),
    )];
    if top < center_y {
        rectangles.push(smithay::utils::Rectangle::new(
            (center_x, top).into(),
            (1, center_y - top).into(),
        ));
    }
    if center_y + 1 < bottom {
        rectangles.push(smithay::utils::Rectangle::new(
            (center_x, center_y + 1).into(),
            (1, bottom - center_y - 1).into(),
        ));
    }
    rectangles
}

impl GpuCursor {
    pub fn new(
        instance: &ash::Instance,
        device: &ash::Device,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self> {
        let buffer = unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(21 * 4)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                None,
            )
        }?;
        let allocation = (|| -> Result<vk::DeviceMemory> {
            let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
            let properties =
                unsafe { instance.get_physical_device_memory_properties(physical_device) };
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
                .context("no coherent cursor upload memory")?;
            let memory = unsafe {
                device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type),
                    None,
                )
            }?;
            let initialize = (|| -> Result<()> {
                unsafe {
                    device.bind_buffer_memory(buffer, memory, 0)?;
                    let mapped =
                        device.map_memory(memory, 0, 21 * 4, vk::MemoryMapFlags::empty())?;
                    let pixels = std::slice::from_raw_parts_mut(mapped.cast::<u8>(), 21 * 4);
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        *pixel = [255, 245, 0, 255];
                    }
                    device.unmap_memory(memory);
                }
                Ok(())
            })();
            if let Err(error) = initialize {
                unsafe {
                    device.free_memory(memory, None);
                }
                return Err(error);
            }
            Ok(memory)
        })();
        match allocation {
            Ok(memory) => Ok(Self {
                device: device.clone(),
                buffer,
                memory,
            }),
            Err(error) => {
                unsafe {
                    device.destroy_buffer(buffer, None);
                }
                Err(error)
            }
        }
    }

    pub unsafe fn draw(
        &self,
        command: vk::CommandBuffer,
        destination: vk::Image,
        size: Size<i32, smithay::utils::Buffer>,
        center: (i32, i32),
    ) {
        let regions = cursor_rectangles(size, center)
            .into_iter()
            .map(|rectangle| {
                vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_offset(vk::Offset3D {
                        x: rectangle.loc.x,
                        y: rectangle.loc.y,
                        z: 0,
                    })
                    .image_extent(vk::Extent3D {
                        width: rectangle.size.w as u32,
                        height: rectangle.size.h as u32,
                        depth: 1,
                    })
            })
            .collect::<Vec<_>>();
        if regions.is_empty() {
            return;
        }
        let barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        unsafe {
            self.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
            self.device.cmd_copy_buffer_to_image(
                command,
                self.buffer,
                destination,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );
        }
    }
}

impl Drop for GpuCursor {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

pub struct SharedImage {
    device: ash::Device,
    pub image: vk::Image,
    memory: vk::DeviceMemory,
    pub dmabuf: Dmabuf,
}

impl SharedImage {
    pub unsafe fn copy_to(
        &self,
        device: &ash::Device,
        command_buffer: vk::CommandBuffer,
        destination: vk::Image,
        queue_family: u32,
    ) {
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let acquire = vk::ImageMemoryBarrier::default()
            .image(self.image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .dst_queue_family_index(queue_family)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        let size = self.dmabuf.size();
        let layers = vk::ImageSubresourceLayers::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .layer_count(1);
        let copy = vk::ImageCopy::default()
            .src_subresource(layers)
            .dst_subresource(layers)
            .extent(vk::Extent3D {
                width: size.w as u32,
                height: size.h as u32,
                depth: 1,
            });
        let release = vk::ImageMemoryBarrier::default()
            .image(self.image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(queue_family)
            .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ);
        unsafe {
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[acquire],
            );
            device.cmd_copy_image(
                command_buffer,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                destination,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            );
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[release],
            );
        }
    }

    pub fn import(
        instance: &ash::Instance,
        device: &ash::Device,
        physical_device: vk::PhysicalDevice,
        dmabuf: Dmabuf,
    ) -> Result<Self> {
        ensure!(
            dmabuf.format().code == Fourcc::Abgr8888,
            "shared image must be RGBA8"
        );
        ensure!(
            dmabuf.format().modifier == Modifier::Linear && dmabuf.num_planes() == 1,
            "shared image must be single-plane linear DMA-BUF"
        );
        let size = dmabuf.size();
        ensure!(size.w > 0 && size.h > 0, "invalid shared image size");
        let mut modifier = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(0)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(vk::Format::R8G8B8A8_SRGB)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC)
            .push_next(&mut modifier)
            .push_next(&mut external);
        let mut external_properties = vk::ExternalImageFormatProperties::default();
        let mut properties =
            vk::ImageFormatProperties2::default().push_next(&mut external_properties);
        unsafe {
            instance.get_physical_device_image_format_properties2(
                physical_device,
                &info,
                &mut properties,
            )
        }
        .context("query shared image format")?;
        ensure!(
            external_properties
                .external_memory_properties
                .external_memory_features
                .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE),
            "driver cannot import this DMA-BUF format"
        );

        let layout = [vk::SubresourceLayout::default()
            .offset(dmabuf.offsets().next().context("missing plane offset")? as u64)
            .row_pitch(dmabuf.strides().next().context("missing plane stride")? as u64)];
        let mut modifier = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(0)
            .plane_layouts(&layout);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_SRGB)
            .extent(vk::Extent3D {
                width: size.w as u32,
                height: size.h as u32,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .push_next(&mut modifier)
            .push_next(&mut external);
        let fd = dmabuf
            .handles()
            .next()
            .context("missing DMA-BUF fd")?
            .try_clone_to_owned()?;
        let fd_api = ash::khr::external_memory_fd::Device::new(instance, device);
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        unsafe {
            fd_api.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd.as_raw_fd(),
                &mut fd_properties,
            )
        }?;
        let image = unsafe { device.create_image(&image_info, None) }
            .context("create imported Vulkan image")?;
        let allocation = (|| -> Result<vk::DeviceMemory> {
            let requirements = unsafe { device.get_image_memory_requirements(image) };
            let compatible = requirements.memory_type_bits & fd_properties.memory_type_bits;
            ensure!(
                compatible != 0,
                "DMA-BUF has no compatible Vulkan memory type"
            );
            let mut import = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                .fd(fd.as_raw_fd());
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let info = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(compatible.trailing_zeros())
                .push_next(&mut import)
                .push_next(&mut dedicated);
            let memory =
                unsafe { device.allocate_memory(&info, None) }.context("import DMA-BUF memory")?;
            std::mem::forget(fd);
            if let Err(error) = unsafe { device.bind_image_memory(image, memory, 0) } {
                unsafe {
                    device.free_memory(memory, None);
                }
                return Err(error.into());
            }
            Ok(memory)
        })();
        match allocation {
            Ok(memory) => Ok(Self {
                device: device.clone(),
                image,
                memory,
                dmabuf,
            }),
            Err(error) => {
                unsafe {
                    device.destroy_image(image, None);
                }
                Err(error)
            }
        }
    }
}

impl Drop for SharedImage {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

impl GpuRenderer {
    pub fn feedback(&self, node: &Path) -> Result<smithay::wayland::dmabuf::DmabufFeedback> {
        use std::os::unix::fs::MetadataExt;
        Ok(smithay::wayland::dmabuf::DmabufFeedbackBuilder::new(
            node.metadata()?.rdev(),
            self.renderer.dmabuf_formats(),
        )
        .build()?)
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        self.renderer.import_dmabuf(dmabuf, None).is_ok()
    }

    pub fn new(node: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .with_context(|| format!("open GPU render node {}", node.display()))?;
        let device = GbmDevice::new(Arc::new(file)).context("create GBM device")?;
        let display =
            unsafe { EGLDisplay::new(device.clone()) }.context("create GBM EGL display")?;
        let context = EGLContext::new(&display).context("create GLES context")?;
        let renderer = unsafe { GlesRenderer::new(context) }.context("create GLES renderer")?;
        let allocator = GbmAllocator::new(device, GbmBufferFlags::RENDERING);
        Ok(Self {
            renderer,
            allocator,
        })
    }

    pub fn capture(
        &mut self,
        surface: &WlSurface,
        size: Size<i32, smithay::utils::Buffer>,
        scale: f64,
    ) -> Result<Dmabuf> {
        let buffer = self.allocator.create_buffer(
            size.w as u32,
            size.h as u32,
            Fourcc::Abgr8888,
            &[Modifier::Linear],
        )?;
        let mut dmabuf = buffer.export()?;
        ensure!(
            dmabuf.format().modifier == Modifier::Linear,
            "GBM did not allocate a linear image: {:?}",
            dmabuf.format()
        );
        ensure!(
            dmabuf.num_planes() == 1,
            "GPU sharing requires a single-plane image"
        );
        let physical_size = Size::<i32, Physical>::from((size.w, size.h));
        let damage = [smithay::utils::Rectangle::from_size(physical_size)];
        let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            render_elements_from_surface_tree(
                &mut self.renderer,
                surface,
                (0, 0),
                scale,
                1.0,
                Kind::Unspecified,
            );
        let mut framebuffer = self.renderer.bind(&mut dmabuf)?;
        let mut frame = self
            .renderer
            .render(&mut framebuffer, physical_size, Transform::Normal)?;
        frame.clear(
            smithay::backend::renderer::Color32F::new(0.08, 0.11, 0.16, 1.0),
            &damage,
        )?;
        draw_render_elements::<GlesRenderer, _, _>(&mut frame, scale, &elements, &damage)?;
        let sync = frame.finish()?;
        sync.wait().context("wait for GLES rendering")?;
        drop(framebuffer);
        Ok(dmabuf)
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

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
                    .position(|properties| {
                        properties.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                    })
                    .context("GPU lacks a graphics queue")?
                        as u32;
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

        pub fn readback(
            &self,
            shared: &SharedImage,
            cursor: Option<(i32, i32)>,
        ) -> Result<Vec<u8>> {
            let size = shared.dmabuf.size();
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
                    .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST);
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
                shared.copy_to(&self.device, command, destination, self.queue_family);
                if let (Some(buffer), Some(center)) = (cursor_buffer.as_ref(), cursor) {
                    buffer.draw(command, destination, size, center);
                }
                let barrier = vk::ImageMemoryBarrier::default()
                    .image(destination)
                    .subresource_range(range)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
                self.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
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
}
