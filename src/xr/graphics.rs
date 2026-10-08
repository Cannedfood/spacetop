use super::*;
use crate::scene::RenderTarget;
use ash::Entry as VkEntry;

pub(super) struct XrEye {
    pub swapchain: xr::Swapchain<xr::Vulkan>,
    pub targets: Vec<RenderTarget>,
    pub extent: vk::Extent2D,
}

impl XrEye {
    pub(super) fn new(
        session: &xr::Session<xr::Vulkan>,
        renderer: &SceneRenderer,
        format: vk::Format,
        view: xr::ViewConfigurationView,
    ) -> Result<Self> {
        let extent = vk::Extent2D {
            width: view.recommended_image_rect_width,
            height: view.recommended_image_rect_height,
        };
        ensure!(
            extent.width > 0 && extent.height > 0,
            "invalid OpenXR eye dimensions"
        );
        let swapchain = session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
                format: format.as_raw() as _,
                sample_count: 1,
                width: extent.width,
                height: extent.height,
                face_count: 1,
                array_size: 1,
                mip_count: 1,
            })
            .context("create eye swapchain")?;
        let targets = swapchain
            .enumerate_images()?
            .into_iter()
            .map(|image| RenderTarget::new(renderer, vk::Image::from_raw(image), extent))
            .collect::<Result<Vec<_>>>()?;
        ensure!(!targets.is_empty(), "OpenXR eye swapchain has no images");
        Ok(Self {
            swapchain,
            targets,
            extent,
        })
    }
}

impl Drop for XrEye {
    fn drop(&mut self) {
        self.targets.clear();
    }
}

pub(super) fn panel_swapchain_format(formats: &[u32]) -> Result<vk::Format> {
    let format = vk::Format::R8G8B8A8_SRGB;
    ensure!(
        formats.contains(&(format.as_raw() as u32)),
        "GPU sharing requires an RGBA8 sRGB OpenXR swapchain"
    );
    Ok(format)
}

pub(super) struct Graphics {
    pub xr_entry: xr::Entry,
    pub vk_entry: VkEntry,
    pub instance: xr::Instance,
    pub system: xr::SystemId,
    pub vk_instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue_family: u32,
    pub queue: vk::Queue,
    pub command_pool: vk::CommandPool,
    pub command_buffer: vk::CommandBuffer,
    pub fence: vk::Fence,
    pub session: xr::Session<xr::Vulkan>,
    pub frame_waiter: xr::FrameWaiter,
    pub frame_stream: xr::FrameStream<xr::Vulkan>,
    pub device_limit: u32,
    pub max_panel_size: u32,
    pub format: vk::Format,
}

impl Graphics {
    pub(super) fn new() -> Result<Self> {
        let xr_entry = unsafe { xr::Entry::load(&()) }.context("load OpenXR loader")?;
        let available = xr_entry
            .enumerate_extensions()
            .context("enumerate OpenXR extensions")?;
        ensure!(
            available.khr_vulkan_enable2,
            "configured runtime does not support XR_KHR_vulkan_enable2"
        );

        let mut extensions = xr::ExtensionSet::default();
        extensions.khr_vulkan_enable2 = true;
        let instance = xr_entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: "spacetop",
                    application_version: 1,
                    engine_name: "spacetop",
                    engine_version: 1,
                    api_version: xr::Version::new(1, 0, 0),
                },
                &extensions,
                &[],
                &(),
            )
            .context("create OpenXR instance")?;
        let system = instance
            .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .context("find HMD system")?;
        ensure!(
            instance
                .enumerate_view_configurations(system)?
                .contains(&VIEW_TYPE),
            "runtime does not support stereo views"
        );
        let blend_modes = instance.enumerate_environment_blend_modes(system, VIEW_TYPE)?;
        ensure!(
            blend_modes.contains(&xr::EnvironmentBlendMode::OPAQUE),
            "runtime does not support opaque blending"
        );

        let requirements = instance.graphics_requirements::<xr::Vulkan>(system)?;
        let mut vk_api_version = vk::make_api_version(
            0,
            requirements.min_api_version_supported.major() as u32,
            requirements.min_api_version_supported.minor() as u32,
            requirements.min_api_version_supported.patch(),
        );
        let vk_entry = unsafe { VkEntry::load() }.context("load Vulkan loader")?;
        let loader_version =
            unsafe { vk_entry.try_enumerate_instance_version() }?.unwrap_or(vk::API_VERSION_1_0);
        ensure!(
            loader_version >= vk::API_VERSION_1_2
                && requirements.max_api_version_supported >= xr::Version::new(1, 2, 0),
            "the OpenXR runtime and Vulkan loader must support Vulkan 1.2"
        );
        vk_api_version = vk_api_version.max(vk::API_VERSION_1_2);
        let vk_app_info = vk::ApplicationInfo::default().api_version(vk_api_version);
        let vk_instance_info = vk::InstanceCreateInfo::default().application_info(&vk_app_info);

        #[allow(clippy::missing_transmute_annotations)]
        let raw_instance = unsafe {
            instance.create_vulkan_instance(
                system,
                std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
                &vk_instance_info as *const _ as *const _,
            )
        }
        .context("runtime Vulkan instance creation")?
        .map_err(|error| anyhow::anyhow!("Vulkan instance creation: {error:?}"))?;
        let vk_instance = unsafe {
            ash::Instance::load(
                vk_entry.static_fn(),
                vk::Instance::from_raw(raw_instance as _),
            )
        };
        let raw_physical_device =
            unsafe { instance.vulkan_graphics_device(system, vk_instance.handle().as_raw() as _) }
                .context("query runtime Vulkan device")?;
        let physical_device = vk::PhysicalDevice::from_raw(raw_physical_device as _);
        ensure!(
            unsafe { vk_instance.get_physical_device_properties(physical_device) }.api_version
                >= vk::API_VERSION_1_2,
            "the OpenXR runtime-selected GPU does not support Vulkan 1.2"
        );
        let queue_family =
            unsafe { vk_instance.get_physical_device_queue_family_properties(physical_device) }
                .iter()
                .position(|properties| properties.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .context("runtime-selected GPU has no graphics queue")? as u32;
        let queue_priorities = [1.0_f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&queue_priorities)];
        ensure!(
            gpu::sharing_supported(&vk_instance, physical_device, vk_api_version)?,
            "GPU sharing requires Vulkan 1.1 and DMA-BUF external-memory, DRM-modifier, DRM-device, image-format-list, and foreign-queue-family support"
        );
        let sharing_extensions = gpu::SHARING_EXTENSIONS.map(|extension| extension.as_ptr());
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&sharing_extensions);
        #[allow(clippy::missing_transmute_annotations)]
        let raw_device = unsafe {
            instance.create_vulkan_device(
                system,
                std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
                physical_device.as_raw() as _,
                &device_info as *const _ as *const _,
            )
        }
        .context("runtime Vulkan device creation")?
        .map_err(|error| anyhow::anyhow!("Vulkan device creation: {error:?}"))?;
        let device = unsafe {
            ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(raw_device as _))
        };
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let command_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }
        .context("create Vulkan command pool")?;
        let command_buffers = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .context("allocate Vulkan command buffer")?;
        let command_buffer = command_buffers[0];
        let fence = unsafe {
            device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )
        }
        .context("create Vulkan fence")?;

        let xr_session = unsafe {
            instance.create_session::<xr::Vulkan>(
                system,
                &xr::vulkan::SessionCreateInfo {
                    instance: vk_instance.handle().as_raw() as _,
                    physical_device: physical_device.as_raw() as _,
                    device: device.handle().as_raw() as _,
                    queue_family_index: queue_family,
                    queue_index: 0,
                },
            )
        }
        .context("create OpenXR session")?;
        let (session, frame_waiter, frame_stream) = xr_session;

        let graphics = instance.system_properties(system)?.graphics_properties;
        let device_limit = unsafe { vk_instance.get_physical_device_properties(physical_device) }
            .limits
            .max_image_dimension2_d;
        let cap = std::env::var("SPACETOP_MAX_PANEL_SIZE")
            .map(|value| {
                value
                    .parse::<u32>()
                    .context("SPACETOP_MAX_PANEL_SIZE must be a positive integer")
            })
            .unwrap_or(Ok(crate::panel::DEFAULT_MAX_PANEL_SIZE))?;
        let max_panel_size = device_limit.min(cap).min(i32::MAX as u32);
        ensure!(
            max_panel_size > 0 && graphics.max_layer_count > 0,
            "invalid Vulkan panel limits, no OpenXR projection layer, or zero SPACETOP_MAX_PANEL_SIZE"
        );
        eprintln!(
            "Vulkan panel limits: {}x{}, one stereo projection layer",
            max_panel_size, max_panel_size
        );
        let formats = session.enumerate_swapchain_formats()?;
        let format = panel_swapchain_format(&formats)?;
        ensure!(
            unsafe {
                vk_instance
                    .get_physical_device_format_properties(physical_device, vk::Format::D32_SFLOAT)
            }
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT),
            "Vulkan GPU lacks D32 depth attachment support"
        );
        Ok(Self {
            xr_entry,
            vk_entry,
            instance,
            system,
            vk_instance,
            physical_device,
            device,
            queue_family,
            queue,
            command_pool,
            command_buffer,
            fence,
            session,
            frame_waiter,
            frame_stream,
            device_limit,
            max_panel_size,
            format,
        })
    }
}
