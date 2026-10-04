//! OpenXR application client. This connects to the configured system runtime;
//! it does not implement an OpenXR runtime or provide inter-application overlay
//! semantics.

use std::{
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::bridge::{CursorState, PanelReceiver, PanelUpdate, XrInput};
use crate::config::{AppConfig, ConfigWatcher};
use crate::gpu::{self, SharedImage};
use crate::panel::{PanelGeometry, PanelLimits, PanelPose, Ray3, dodge_windows};
use crate::scene::{PanelTexture, RenderTarget, SceneFrame, SceneRenderer, SkyboxTexture};
use anyhow::{Context, Result, ensure};
use ash::{
    Entry as VkEntry,
    vk::{self, Handle},
};
use openxr as xr;
use smithay::backend::allocator::Buffer;

struct XrPanel {
    texture: PanelTexture,
    geometry: PanelGeometry,
    saved_pose: PanelPose,
    temporary_pose: PanelPose,
    resize_pending: bool,
    is_fullscreen: bool,
    fullscreen_anchor_pose: Option<PanelPose>,
}

struct XrEye {
    swapchain: xr::Swapchain<xr::Vulkan>,
    targets: Vec<RenderTarget>,
    extent: vk::Extent2D,
}

struct PendingSkybox {
    image: String,
    worker: JoinHandle<Result<SkyboxTexture>>,
}

impl PendingSkybox {
    fn new(
        scene: &SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        image: String,
    ) -> Result<Self> {
        let worker = SkyboxTexture::load_async(scene, instance, physical_device, &image)?;
        Ok(Self { image, worker })
    }
}

impl XrEye {
    fn new(
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

type PanelImages = std::collections::BTreeMap<u64, XrPanel>;

fn reconcile_panel_geometry(
    current: &mut PanelGeometry,
    saved_pose: &mut PanelPose,
    temporary_pose: &mut PanelPose,
    committed: PanelGeometry,
    resizing: bool,
) {
    if resizing {
        *current = committed;
        *saved_pose = committed.pose;
        *temporary_pose = committed.pose;
        return;
    }
    let animated_pose = current.pose;
    *current = committed;
    current.pose = animated_pose;
    saved_pose.width_m = committed.pose.width_m;
    temporary_pose.width_m = committed.pose.width_m;
}

fn update_dodge_targets(
    panels: &mut PanelImages,
    fixed_windows: &[u64],
    player: glam::Vec3,
    margin_m: f32,
) {
    if panels.values().any(|panel| panel.is_fullscreen) {
        return;
    }
    let geometries: Vec<_> = panels
        .iter()
        .map(|(id, panel)| {
            let pose = if fixed_windows.contains(id) {
                panel.geometry.pose
            } else {
                panel.saved_pose
            };
            (
                *id,
                PanelGeometry {
                    pose,
                    ..panel.geometry
                },
            )
        })
        .collect();
    let targets = dodge_windows(&geometries, fixed_windows, player, margin_m);
    for (id, panel) in panels {
        if let Some(pose) = targets.get(id) {
            panel.temporary_pose = *pose;
        }
    }
}

fn save_dodge_targets(panels: &mut PanelImages, input: &crate::bridge::InputSender) -> Result<()> {
    save_dodge_targets_except(panels, input, None)
}

fn save_dodge_targets_except(
    panels: &mut PanelImages,
    input: &crate::bridge::InputSender,
    excluded_panel: Option<u64>,
) -> Result<()> {
    if panels.values().any(|panel| panel.is_fullscreen) {
        return Ok(());
    }
    for (panel_id, panel) in panels {
        if Some(*panel_id) == excluded_panel {
            continue;
        }
        panel.resize_pending = false;
        panel.saved_pose = panel.temporary_pose;
        input.send(XrInput::MovePanel {
            panel_id: *panel_id,
            pose: panel.saved_pose,
        })?;
    }
    Ok(())
}

fn finish_resize(
    panels: &mut PanelImages,
    input: &crate::bridge::InputSender,
    resizing_panel: &mut Option<u64>,
    geometry: Option<PanelGeometry>,
    edges: [bool; 4],
    requested_size: Option<(i32, i32)>,
) -> Result<()> {
    if let Some(panel_id) = resizing_panel.take() {
        if let Some((geometry, (width, height))) = geometry.zip(requested_size) {
            input.send(XrInput::ResizePanel {
                panel_id,
                width,
                height,
                anchor: Some((geometry, edges)),
            })?;
        }
        save_dodge_targets_except(panels, input, Some(panel_id))?;
    }
    Ok(())
}

fn smooth_pose(current: PanelPose, target: PanelPose, factor: f32) -> PanelPose {
    let angle_delta = |from: f32, to: f32| PanelPose::wrap_angle(to - from);
    PanelPose {
        center: current.center.lerp(target.center, factor),
        yaw: current.yaw + angle_delta(current.yaw, target.yaw) * factor,
        pitch: current.pitch + (target.pitch - current.pitch) * factor,
        width_m: current.width_m + (target.width_m - current.width_m) * factor,
    }
}

impl Drop for XrEye {
    fn drop(&mut self) {
        self.targets.clear();
    }
}

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

fn tracked_floor_height(previous: f32, location: xr::SpaceLocation) -> f32 {
    let height = location.pose.position.y;
    if location
        .location_flags
        .contains(xr::SpaceLocationFlags::POSITION_VALID)
        && height.is_finite()
    {
        height
    } else {
        previous
    }
}

fn cursor_pose(
    ray: Ray3,
    player: glam::Vec3,
    panels: impl Iterator<Item = PanelGeometry>,
    sphere_radius: &mut f32,
) -> Option<PanelPose> {
    let direction = ray.direction.try_normalize()?;
    let ray = Ray3 { direction, ..ray };
    let nearest = panels
        .filter_map(|geometry| geometry.intersect(ray).map(|hit| (geometry.pose, hit)))
        .min_by(|(_, first), (_, second)| first.distance_m.total_cmp(&second.distance_m));
    if let Some((pose, hit)) = nearest {
        let center = ray.origin + direction * hit.distance_m;
        *sphere_radius = center.distance(player);
        return Some(PanelPose {
            center,
            width_m: 0.021,
            ..pose
        });
    }
    let offset = ray.origin - player;
    let projection = offset.dot(direction);
    let discriminant =
        projection * projection - offset.length_squared() + *sphere_radius * *sphere_radius;
    if discriminant < 0.0 {
        return None;
    }
    let distance = -projection + discriminant.sqrt();
    if distance < 0.0 {
        return None;
    }
    Some(PanelPose {
        width_m: 0.021,
        ..PanelPose::looking_from_to(ray.origin + direction * distance, player)
    })
}

fn panel_swapchain_format(formats: &[u32]) -> Result<vk::Format> {
    let format = vk::Format::R8G8B8A8_SRGB;
    ensure!(
        formats.contains(&(format.as_raw() as u32)),
        "GPU sharing requires an RGBA8 sRGB OpenXR swapchain"
    );
    Ok(format)
}

/// Run the OpenXR client loop until the runtime requests session/application exit.
///
/// It renders captured Wayland panels into a Vulkan stereo projection layer and
/// forwards the right-hand aim/select controller actions to the compositor.
pub fn run(
    frames: PanelReceiver,
    input: crate::bridge::InputSender,
    mut config: AppConfig,
) -> Result<()> {
    let mut config_watcher = ConfigWatcher::new_default()?;
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

    // OpenXR creates the Vulkan instance/device with runtime-required extensions.
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
    let device =
        unsafe { ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(raw_device as _)) };
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
    let (session, mut frame_waiter, mut frame_stream) = xr_session;

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
        .unwrap_or(Ok(4096))?;
    let limits = PanelLimits {
        max_width: device_limit.min(cap).min(i32::MAX as u32),
        max_height: device_limit.min(cap).min(i32::MAX as u32),
        max_layers: u32::MAX,
    };
    ensure!(
        limits.max_width > 0 && limits.max_height > 0 && graphics.max_layer_count > 0,
        "invalid Vulkan panel limits, no OpenXR projection layer, or zero SPACETOP_MAX_PANEL_SIZE"
    );
    eprintln!(
        "Vulkan panel limits: {}x{}, one stereo projection layer",
        limits.max_width, limits.max_height
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
    let mut scene = SceneRenderer::new(&device, &vk_instance, physical_device, format, &config)?;
    let mut skybox = None;
    let mut pending_skybox = Some(PendingSkybox::new(
        &scene,
        &vk_instance,
        physical_device,
        config.background.image.clone(),
    )?);
    let mut background_exposure = 0.0_f32;
    let mut background_exposure_target = 0.0_f32;
    scene.set_background_exposure(background_exposure, &config)?;
    let mut queued_skybox_image = None;
    let view_configuration = instance.enumerate_view_configuration_views(system, VIEW_TYPE)?;
    ensure!(
        view_configuration.len() == 2,
        "runtime must provide two stereo views"
    );
    let mut eyes = view_configuration
        .into_iter()
        .map(|view| {
            ensure!(
                view.recommended_image_rect_width <= device_limit
                    && view.recommended_image_rect_height <= device_limit,
                "OpenXR eye dimensions exceed Vulkan device limits"
            );
            XrEye::new(&session, &scene, format, view)
        })
        .collect::<Result<Vec<_>>>()?;
    let render_node = gpu::render_node(&vk_instance, physical_device)
        .context("GPU sharing requires a DRM render node for the runtime-selected Vulkan GPU")?;
    input
        .send(XrInput::GpuDevice {
            render_node,
            limits,
        })
        .context("request GPU compositing")?;
    let space =
        session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
    let stage_space = if session
        .enumerate_reference_spaces()?
        .contains(&xr::ReferenceSpaceType::STAGE)
    {
        match session.create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY) {
            Ok(stage) => Some(stage),
            Err(error) => {
                eprintln!(
                    "OpenXR STAGE floor unavailable: {error}; using fallback floor height {}m",
                    config.floor.height_m
                );
                None
            }
        }
    } else {
        eprintln!(
            "OpenXR STAGE unsupported; using fallback floor height {}m",
            config.floor.height_m
        );
        None
    };
    let mut floor_y = config.floor.height_m;
    let mut stage_floor_calibrated = false;
    let action_set = instance.create_action_set("spacetop", "Spacetop input", 0)?;
    let right_hand = instance.string_to_path("/user/hand/right")?;
    let left_hand = instance.string_to_path("/user/hand/left")?;
    let aim_action =
        action_set.create_action::<xr::Posef>("aim_pose", "Aim pose", &[right_hand])?;
    let trigger_action = action_set.create_action::<bool>("trigger", "Trigger", &[right_hand])?;
    let launcher_action =
        action_set.create_action::<bool>("launcher", "Launcher toggle", &[right_hand])?;
    let face_click_action = action_set.create_action::<bool>(
        "face_click",
        "Face button click",
        &[left_hand, right_hand],
    )?;
    let stick_click_action =
        action_set.create_action::<bool>("stick_click", "Thumbstick click", &[right_hand])?;
    let grip_action = action_set.create_action::<bool>("grip", "Grip", &[right_hand])?;
    let stick_action =
        action_set.create_action::<xr::Vector2f>("stick", "Thumbstick", &[right_hand])?;
    let aim_path = instance.string_to_path("/user/hand/right/input/aim/pose")?;
    for (profile, button, launcher, left_face, right_face, grip, stick, stick_click) in [
        (
            "/interaction_profiles/khr/simple_controller",
            "select/click",
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "/interaction_profiles/oculus/touch_controller",
            "trigger/value",
            Some("b/click"),
            Some("x/click"),
            Some("a/click"),
            Some("squeeze/value"),
            Some("thumbstick"),
            Some("thumbstick/click"),
        ),
        (
            "/interaction_profiles/valve/index_controller",
            "trigger/click",
            Some("b/click"),
            Some("a/click"),
            Some("a/click"),
            Some("squeeze/value"),
            Some("thumbstick"),
            Some("thumbstick/click"),
        ),
        (
            "/interaction_profiles/htc/vive_controller",
            "trigger/click",
            Some("trackpad/click"),
            None,
            None,
            Some("squeeze/click"),
            Some("trackpad"),
            None,
        ),
        (
            "/interaction_profiles/microsoft/motion_controller",
            "trigger/value",
            Some("trackpad/click"),
            None,
            None,
            Some("squeeze/click"),
            Some("thumbstick"),
            Some("thumbstick/click"),
        ),
    ] {
        let mut bindings = vec![
            xr::Binding::new(&aim_action, aim_path),
            xr::Binding::new(
                &trigger_action,
                instance.string_to_path(&format!("/user/hand/right/input/{button}"))?,
            ),
        ];
        if let Some(launcher) = launcher {
            bindings.push(xr::Binding::new(
                &launcher_action,
                instance.string_to_path(&format!("/user/hand/right/input/{launcher}"))?,
            ));
        }
        if let Some(left_face) = left_face {
            bindings.push(xr::Binding::new(
                &face_click_action,
                instance.string_to_path(&format!("/user/hand/left/input/{left_face}"))?,
            ));
        }
        if let Some(right_face) = right_face {
            bindings.push(xr::Binding::new(
                &face_click_action,
                instance.string_to_path(&format!("/user/hand/right/input/{right_face}"))?,
            ));
        }
        if let Some(grip) = grip {
            bindings.push(xr::Binding::new(
                &grip_action,
                instance.string_to_path(&format!("/user/hand/right/input/{grip}"))?,
            ));
        }
        if let Some(stick) = stick {
            bindings.push(xr::Binding::new(
                &stick_action,
                instance.string_to_path(&format!("/user/hand/right/input/{stick}"))?,
            ));
        }
        if let Some(stick_click) = stick_click {
            bindings.push(xr::Binding::new(
                &stick_click_action,
                instance.string_to_path(&format!("/user/hand/right/input/{stick_click}"))?,
            ));
        }
        instance
            .suggest_interaction_profile_bindings(instance.string_to_path(profile)?, &bindings)?;
    }
    session.attach_action_sets(&[&action_set])?;
    let aim_space = aim_action.create_space(&session, right_hand, xr::Posef::IDENTITY)?;
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut exit = false;
    let mut panel_frames = PanelImages::new();
    let mut active_fullscreen_panel = None;
    let mut pending_spawn = std::collections::BTreeSet::new();
    let mut cursor_ray: Option<Ray3> = None;
    let mut cursor_sphere_radius = config.window.default_distance_m;
    let mut pointer_tracked = false;
    let mut left_click_pressed = false;
    let mut middle_click_pressed = false;
    let mut stick_click_consumed = false;
    let mut launcher_pressed = false;
    let mut grabbed_panel: Option<u64> = None;
    let mut grab_radius = config.window.default_distance_m;
    let mut grab_player_position = glam::Vec3::ZERO;
    let mut grab_initial_radius = config.window.default_distance_m;
    let mut grab_initial_width = 1.0_f32;
    let mut grab_direction_offset = glam::Vec2::ZERO;
    let mut resizing_panel = None;
    let mut resize_geometry = None;
    let mut resize_initial_size = (0_i32, 0_i32);
    let mut resize_initial_hit = glam::Vec2::ZERO;
    let mut resize_edges = [false; 4];
    let mut resize_requested_size = None;
    let mut cursor_close_panel = None;
    let mut timings = crate::timing::Timings::new();
    let mut last_config_check = Instant::now();
    let mut rendered_frame_index = 0_u32;
    let mut mouse_cursor_state: Option<CursorState> = None;

    while !exit {
        if pending_skybox
            .as_ref()
            .is_some_and(|pending| pending.worker.is_finished())
        {
            let pending = pending_skybox.take().expect("finished skybox load exists");
            match pending.worker.join() {
                Ok(Ok(loaded)) if pending.image == config.background.image => {
                    scene.update_skybox_diffuse(loaded.diffuse_irradiance(), &config)?;
                    skybox = Some(loaded);
                    background_exposure_target = 1.0;
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    eprintln!("skybox load failed: {error:#}");
                    background_exposure_target = 1.0;
                }
                Err(_) => {
                    eprintln!("skybox loading thread panicked");
                    background_exposure_target = 1.0;
                }
            }
            if let Some(image) = queued_skybox_image.take() {
                pending_skybox = Some(PendingSkybox::new(
                    &scene,
                    &vk_instance,
                    physical_device,
                    image,
                )?);
                background_exposure_target = 0.0;
            }
        }
        if last_config_check.elapsed() >= Duration::from_millis(250) {
            last_config_check = Instant::now();
            if let Some(reload) = config_watcher.reload_if_changed() {
                match reload {
                    Ok(next_config) => {
                        let result: Result<Option<String>> = (|| {
                            let next_skybox_image = (next_config.background.image
                                != config.background.image)
                                .then(|| next_config.background.image.clone());
                            scene.update_config(&next_config)?;
                            Ok(next_skybox_image)
                        })();
                        match result {
                            Ok(next_skybox_image) => {
                                if let Some(image) = next_skybox_image {
                                    if pending_skybox.is_some() {
                                        queued_skybox_image = Some(image);
                                        background_exposure_target = 0.0;
                                    } else {
                                        match PendingSkybox::new(
                                            &scene,
                                            &vk_instance,
                                            physical_device,
                                            image,
                                        ) {
                                            Ok(pending) => {
                                                pending_skybox = Some(pending);
                                                background_exposure_target = 0.0;
                                            }
                                            Err(error) => {
                                                eprintln!("skybox load could not start: {error:#}");
                                                background_exposure_target = 1.0;
                                            }
                                        }
                                    }
                                }
                                if stage_space.is_none() || !stage_floor_calibrated {
                                    floor_y = next_config.floor.height_m;
                                }
                                cursor_sphere_radius = next_config.window.default_distance_m;
                                if grabbed_panel.is_none() {
                                    grab_radius = next_config.window.default_distance_m;
                                    grab_initial_radius = next_config.window.default_distance_m;
                                }
                                let window_config_changed = next_config.window.default_distance_m
                                    != config.window.default_distance_m
                                    || next_config.window.default_vertical_angle_degrees
                                        != config.window.default_vertical_angle_degrees
                                    || next_config.window.pixels_per_degree
                                        != config.window.pixels_per_degree
                                    || next_config.window.display_scale
                                        != config.window.display_scale;
                                if window_config_changed
                                    && let Err(error) = input.send(XrInput::ConfigReloaded {
                                        default_window_distance: next_config
                                            .window
                                            .default_distance_m,
                                        default_vertical_angle_degrees: next_config
                                            .window
                                            .default_vertical_angle_degrees,
                                        window_pixels_per_degree: next_config
                                            .window
                                            .pixels_per_degree,
                                        window_display_scale: next_config.window.display_scale,
                                    })
                                {
                                    eprintln!(
                                        "failed to notify compositor of config reload: {error}"
                                    );
                                }
                                config = next_config;
                                eprintln!("Reloaded configuration");
                            }
                            Err(error) => {
                                eprintln!("configuration reload rejected: {error:#}");
                            }
                        }
                    }
                    Err(error) => eprintln!("configuration reload rejected: {error:#}"),
                }
            }
        }
        while let Some(event) = timings.measure("openxr/poll-event", Duration::ZERO, || {
            instance.poll_event(&mut events)
        })? {
            match event {
                xr::Event::SessionStateChanged(changed)
                    if changed.session() == session.as_raw() =>
                {
                    match changed.state() {
                        xr::SessionState::READY => {
                            timings.measure(
                                "openxr/begin-session",
                                Duration::from_millis(250),
                                || session.begin(VIEW_TYPE),
                            )?;
                            running = true;
                        }
                        xr::SessionState::STOPPING => {
                            input.send(XrInput::PointerLost { time_ms: 0 })?;
                            pointer_tracked = false;
                            left_click_pressed = false;
                            middle_click_pressed = false;
                            launcher_pressed = false;
                            running = false;
                            timings.measure(
                                "openxr/end-session",
                                Duration::from_millis(250),
                                || session.end(),
                            )?;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => exit = true,
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => exit = true,
                _ => {}
            }
        }
        // Drain compositor-to-XR panel snapshots without blocking the XR frame loop.
        timings.reset_external();
        let updates_started = Instant::now();
        if let Some(cursor) = frames.take_cursor() {
            mouse_cursor_state = Some(cursor);
        }
        let updates = frames.drain();
        for update in &updates {
            if let PanelUpdate::Removed { panel_id } = update {
                timings.measure("mixed/panel-retire", Duration::ZERO, || {
                    panel_frames.remove(panel_id);
                });
                pending_spawn.remove(panel_id);
                if grabbed_panel == Some(*panel_id) {
                    grabbed_panel = None;
                }
                if active_fullscreen_panel == Some(*panel_id) {
                    active_fullscreen_panel = panel_frames
                        .iter()
                        .rev()
                        .find(|(_, panel)| panel.is_fullscreen)
                        .map(|(id, _)| *id);
                    if active_fullscreen_panel.is_none() {
                        for panel in panel_frames.values_mut() {
                            panel.geometry.pose = panel.saved_pose;
                            panel.temporary_pose = panel.saved_pose;
                        }
                    }
                }
            }
        }
        for command in updates {
            match command {
                PanelUpdate::GpuFrame {
                    panel_id,
                    dmabuf,
                    geometry,
                    is_fullscreen,
                } => {
                    let shared = timings
                        .measure("gpu/dmabuf-import", Duration::ZERO, || {
                            SharedImage::import(&vk_instance, &device, physical_device, dmabuf)
                        })
                        .context("mandatory GPU DMA-BUF import failed")?;
                    let size = shared.dmabuf.size();
                    ensure!(
                        size.w > 0
                            && size.h > 0
                            && size.w as u32 <= limits.max_width
                            && size.h as u32 <= limits.max_height,
                        "window image exceeds negotiated Vulkan limits"
                    );
                    let texture = timings.measure("gpu/panel-texture", Duration::ZERO, || {
                        PanelTexture::new(&scene, shared)
                    })?;
                    timings.measure("gpu/panel-replace", Duration::ZERO, || {
                        if let Some(panel) = panel_frames.get_mut(&panel_id) {
                            let fullscreen_changed = panel.is_fullscreen != is_fullscreen;
                            reconcile_panel_geometry(
                                &mut panel.geometry,
                                &mut panel.saved_pose,
                                &mut panel.temporary_pose,
                                geometry,
                                panel.resize_pending && panel.is_fullscreen == is_fullscreen,
                            );
                            panel.texture = texture;
                            panel.is_fullscreen = is_fullscreen;
                            if fullscreen_changed {
                                panel.fullscreen_anchor_pose = None;
                                if is_fullscreen {
                                    active_fullscreen_panel = Some(panel_id);
                                } else if active_fullscreen_panel == Some(panel_id) {
                                    active_fullscreen_panel = panel_frames
                                        .iter()
                                        .rev()
                                        .find(|(_, candidate)| candidate.is_fullscreen)
                                        .map(|(id, _)| *id);
                                    if active_fullscreen_panel.is_none() {
                                        for panel in panel_frames.values_mut() {
                                            panel.geometry.pose = panel.saved_pose;
                                            panel.temporary_pose = panel.saved_pose;
                                        }
                                    }
                                }
                                if is_fullscreen {
                                    grabbed_panel = None;
                                    resizing_panel = None;
                                    resize_geometry = None;
                                    resize_requested_size = None;
                                }
                            }
                        } else {
                            panel_frames.insert(
                                panel_id,
                                XrPanel {
                                    texture,
                                    geometry,
                                    saved_pose: geometry.pose,
                                    temporary_pose: geometry.pose,
                                    resize_pending: false,
                                    is_fullscreen,
                                    fullscreen_anchor_pose: None,
                                },
                            );
                            if is_fullscreen {
                                active_fullscreen_panel = Some(panel_id);
                            }
                            pending_spawn.insert(panel_id);
                        }
                    });
                }
                PanelUpdate::Removed { .. } => {}
            }
        }
        timings.record(
            "app/panel-updates",
            timings.app_elapsed(updates_started.elapsed()),
            Duration::ZERO,
        );
        timings.record_external("openxr/panel-updates", "gpu/panel-updates", Duration::ZERO);
        if exit {
            break;
        }
        if !running {
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        let wait_started = Instant::now();
        let frame_state = frame_waiter.wait()?;
        let period =
            Duration::from_nanos(frame_state.predicted_display_period.as_nanos().max(1) as u64);
        if frame_state.should_render {
            timings.record("openxr/wait-frame", wait_started.elapsed(), period * 3);
        }
        timings.reset_external();
        let active_started = Instant::now();
        if frame_state.should_render {
            input.request_frame()?;
        }
        let (view_state, views) = timings.measure("openxr/locate-views", Duration::ZERO, || {
            session.locate_views(VIEW_TYPE, frame_state.predicted_display_time, &space)
        })?;
        if let Some(stage) = &stage_space {
            let location = timings.measure("openxr/locate-floor", Duration::ZERO, || {
                stage.locate(&space, frame_state.predicted_display_time)
            })?;
            if location
                .location_flags
                .contains(xr::SpaceLocationFlags::POSITION_VALID)
                && location.pose.position.y.is_finite()
            {
                stage_floor_calibrated = true;
            }
            floor_y = tracked_floor_height(floor_y, location);
        }
        let mut gaze_ray = None;
        if view_state.contains(xr::ViewStateFlags::POSITION_VALID) && !views.is_empty() {
            grab_player_position = views
                .iter()
                .map(|view| {
                    glam::Vec3::new(
                        view.pose.position.x,
                        view.pose.position.y,
                        view.pose.position.z,
                    )
                })
                .sum::<glam::Vec3>()
                / views.len() as f32;
            let orientation = views[0].pose.orientation;
            let orientation =
                glam::Quat::from_xyzw(orientation.x, orientation.y, orientation.z, orientation.w);
            let look_direction = orientation * glam::Vec3::NEG_Z;
            let mut fullscreen_look_direction = None;
            if view_state.contains(xr::ViewStateFlags::ORIENTATION_VALID) {
                fullscreen_look_direction = Some(look_direction);
                gaze_ray = Some(Ray3 {
                    origin: grab_player_position,
                    direction: look_direction,
                });
            }
            for panel_id in std::mem::take(&mut pending_spawn) {
                if let Some(panel) = panel_frames.get_mut(&panel_id) {
                    let mut pose = PanelPose::on_sphere_from_aim(
                        look_direction,
                        glam::Vec2::new(
                            0.0,
                            config.window.default_vertical_angle_degrees.to_radians(),
                        ),
                        config.window.default_distance_m,
                        grab_player_position,
                    );
                    pose.width_m = panel.saved_pose.width_m;
                    panel.saved_pose = pose;
                    panel.temporary_pose = pose;
                    panel.geometry.pose = pose;
                    update_dodge_targets(
                        &mut panel_frames,
                        &[panel_id],
                        grab_player_position,
                        config.window.collision_margin_m,
                    );
                    save_dodge_targets(&mut panel_frames, &input)?;
                }
            }
            if let (Some(panel_id), Some(look_direction)) =
                (active_fullscreen_panel, fullscreen_look_direction)
                && let Some(panel) = panel_frames.get_mut(&panel_id)
                && panel.fullscreen_anchor_pose.is_none()
            {
                panel.fullscreen_anchor_pose = Some(PanelPose::looking_from_to(
                    grab_player_position
                        + look_direction.normalize_or_zero() * config.window.default_distance_m,
                    grab_player_position,
                ));
            }
        }
        // Update the action set and forward the right controller's aim ray.
        timings.measure("openxr/sync-actions", Duration::ZERO, || {
            session.sync_actions(&[xr::ActiveActionSet::new(&action_set)])
        })?;
        let mut tracked_this_frame = false;
        let time_ms = (frame_state.predicted_display_time.as_nanos() / 1_000_000) as u32;
        if timings.measure("openxr/action-state", Duration::ZERO, || {
            aim_action.is_active(&session, right_hand)
        })? {
            let controller = timings.measure("openxr/locate-controller", Duration::ZERO, || {
                aim_space.locate(&space, frame_state.predicted_display_time)
            })?;
            if controller
                .location_flags
                .contains(xr::SpaceLocationFlags::POSITION_VALID)
                && controller
                    .location_flags
                    .contains(xr::SpaceLocationFlags::ORIENTATION_VALID)
            {
                tracked_this_frame = true;
                let pose = controller.pose;
                let orientation = glam::Quat::from_xyzw(
                    pose.orientation.x,
                    pose.orientation.y,
                    pose.orientation.z,
                    pose.orientation.w,
                );
                let direction = orientation * glam::Vec3::NEG_Z;
                cursor_ray = Some(Ray3 {
                    origin: glam::Vec3::new(pose.position.x, pose.position.y, pose.position.z),
                    direction,
                });
                let _ = input.try_send(XrInput::Ray {
                    ray: cursor_ray.expect("ray assigned above"),
                    gaze_ray,
                    time_ms,
                });
                let resize_reach_px = config.window.grab_reach_px();
                let nearest_proximity = cursor_ray.and_then(|ray| {
                    panel_frames
                        .iter()
                        .filter(|(id, _)| {
                            active_fullscreen_panel.is_none_or(|fullscreen| **id == fullscreen)
                        })
                        .filter_map(|(id, panel)| {
                            let hit = panel.geometry.intersect_unbounded(ray)?;
                            let width = panel.geometry.logical_size.w as f32;
                            let height = panel.geometry.logical_size.h as f32;
                            let outside_x =
                                (-hit.surface_px.x).max(hit.surface_px.x - width).max(0.0);
                            let outside_y =
                                (-hit.surface_px.y).max(hit.surface_px.y - height).max(0.0);
                            let within_grab_reach =
                                outside_x <= resize_reach_px && outside_y <= resize_reach_px;
                            let within_cursor_proximity = outside_x.hypot(outside_y)
                                <= config.window.cursor_proximity_radius_px;
                            (within_grab_reach || within_cursor_proximity).then_some((*id, hit))
                        })
                        .min_by(|(_, first), (_, second)| {
                            first.distance_m.total_cmp(&second.distance_m)
                        })
                });
                let hovered_panel = nearest_proximity.map(|(id, _)| id);
                cursor_close_panel = nearest_proximity
                    .filter(|(id, hit)| {
                        let Some(panel) = panel_frames.get(id) else {
                            return false;
                        };
                        let width = panel.geometry.logical_size.w as f32;
                        let height = panel.geometry.logical_size.h as f32;
                        hit.surface_px
                            .x
                            .min(width - hit.surface_px.x)
                            .min(hit.surface_px.y)
                            .min(height - hit.surface_px.y)
                            <= config.window.cursor_proximity_radius_px
                    })
                    .map(|(id, hit)| (id, hit.surface_px));
                let pointing_at_window = hovered_panel.is_some();
                let trigger = timings.measure("openxr/action-state", Duration::ZERO, || {
                    trigger_action.state(&session, right_hand)
                })?;
                let grip = timings.measure("openxr/action-state", Duration::ZERO, || {
                    grip_action.state(&session, right_hand)
                })?;
                if grip.changed_since_last_sync {
                    if grip.current_state {
                        grabbed_panel = cursor_ray.and_then(|ray| {
                            panel_frames
                                .iter()
                                .filter(|(id, _)| {
                                    active_fullscreen_panel
                                        .is_none_or(|fullscreen| **id == fullscreen)
                                })
                                .filter_map(|(id, panel)| {
                                    panel
                                        .geometry
                                        .intersect_with_margin_px(
                                            ray,
                                            config.window.grab_reach_px(),
                                        )
                                        .map(|hit| (*id, hit))
                                })
                                .min_by(|(_, first), (_, second)| {
                                    first.distance_m.total_cmp(&second.distance_m)
                                })
                                .map(|(id, _hit)| {
                                    let panel =
                                        panel_frames.get_mut(&id).expect("hit panel exists");
                                    panel.resize_pending = false;
                                    grab_radius = panel
                                        .geometry
                                        .pose
                                        .center
                                        .distance(grab_player_position)
                                        .clamp(0.6, 5.0);
                                    grab_initial_radius = grab_radius;
                                    grab_initial_width = panel.geometry.pose.width_m;
                                    let aim_angles = PanelPose::spherical_angles(ray.direction);
                                    let center_angles = PanelPose::spherical_angles(
                                        panel.geometry.pose.center - grab_player_position,
                                    );
                                    grab_direction_offset = glam::Vec2::new(
                                        PanelPose::wrap_angle(center_angles.x - aim_angles.x),
                                        center_angles.y - aim_angles.y,
                                    );
                                    id
                                })
                        });
                    } else {
                        if grabbed_panel.take().is_some() {
                            save_dodge_targets(&mut panel_frames, &input)?;
                        }
                    }
                }
                if trigger.changed_since_last_sync
                    && trigger.current_state
                    && grabbed_panel.is_none()
                    && let Some(panel_id) = hovered_panel
                    && active_fullscreen_panel.is_none_or(|fullscreen| fullscreen == panel_id)
                    && let Some(panel) = panel_frames.get(&panel_id)
                    && let Some(ray) = cursor_ray
                    && let Some(hit) = panel
                        .geometry
                        .intersect_with_margin_px(ray, resize_reach_px)
                {
                    resize_edges = panel.geometry.resize_edges_from_hit(hit);
                    if resize_edges.into_iter().any(|edge| edge) {
                        resizing_panel = Some(panel_id);
                        resize_geometry = Some(panel.geometry);
                        resize_initial_size =
                            (panel.geometry.logical_size.w, panel.geometry.logical_size.h);
                        resize_initial_hit = hit.surface_px;
                        resize_requested_size = None;
                        if let Some(panel) = panel_frames.get_mut(&panel_id) {
                            panel.resize_pending = true;
                        }
                    }
                }
                if trigger.changed_since_last_sync && !trigger.current_state {
                    finish_resize(
                        &mut panel_frames,
                        &input,
                        &mut resizing_panel,
                        resize_geometry,
                        resize_edges,
                        resize_requested_size,
                    )?;
                    resize_geometry = None;
                    resize_requested_size = None;
                }
                if trigger.is_active
                    && trigger.current_state
                    && let Some(panel_id) = resizing_panel
                    && let Some(geometry) = resize_geometry
                    && let Some(ray) = cursor_ray
                    && let Some(hit) = geometry.intersect_unbounded(ray)
                {
                    let width = if resize_edges[0] {
                        resize_initial_size.0 as f32 - (hit.surface_px.x - resize_initial_hit.x)
                    } else if resize_edges[1] {
                        resize_initial_size.0 as f32 + (hit.surface_px.x - resize_initial_hit.x)
                    } else {
                        resize_initial_size.0 as f32
                    };
                    let height = if resize_edges[2] {
                        resize_initial_size.1 as f32 - (hit.surface_px.y - resize_initial_hit.y)
                    } else if resize_edges[3] {
                        resize_initial_size.1 as f32 + (hit.surface_px.y - resize_initial_hit.y)
                    } else {
                        resize_initial_size.1 as f32
                    };
                    let new_size = (
                        width.round().max(1.0) as i32,
                        height.round().max(1.0) as i32,
                    );
                    if resize_requested_size != Some(new_size) {
                        resize_requested_size = Some(new_size);
                        let _ = input.try_send(XrInput::ResizePanel {
                            panel_id,
                            width: new_size.0,
                            height: new_size.1,
                            anchor: Some((geometry, resize_edges)),
                        });
                    }
                }
                let delta_seconds = (frame_state.predicted_display_period.as_nanos() as f32
                    / 1_000_000_000.0)
                    .clamp(0.0, 0.1);
                if let Some(panel_id) = grabbed_panel {
                    let stick = timings
                        .measure("openxr/action-state", Duration::ZERO, || {
                            stick_action.state(&session, right_hand)
                        })?
                        .current_state;
                    grab_radius = (grab_radius + stick.y * delta_seconds * 1.5).clamp(0.6, 5.0);
                    if let Some(ray) = cursor_ray
                        && let Some(panel) = panel_frames.get_mut(&panel_id)
                    {
                        let mut pose = PanelPose::on_sphere_from_aim(
                            ray.direction,
                            grab_direction_offset,
                            grab_radius,
                            grab_player_position,
                        );
                        pose.width_m = PanelPose::width_for_distance(
                            grab_initial_width,
                            grab_initial_radius,
                            grab_radius,
                        );
                        panel.geometry.pose = pose;
                        let _ = input.try_send(XrInput::MovePanel { panel_id, pose });
                    }
                } else if pointing_at_window {
                    let stick = timings
                        .measure("openxr/action-state", Duration::ZERO, || {
                            stick_action.state(&session, right_hand)
                        })?
                        .current_state;
                    if stick.y.abs() > 0.15 {
                        let scroll_input = stick.y.powf(3.0);
                        let _ = input.try_send(XrInput::Scroll {
                            value: -f64::from(scroll_input) * delta_seconds as f64 * 1800.0,
                            time_ms,
                        });
                    }
                }
                let left_face = timings.measure("openxr/action-state", Duration::ZERO, || {
                    face_click_action.state(&session, left_hand)
                })?;
                let right_face = timings.measure("openxr/action-state", Duration::ZERO, || {
                    face_click_action.state(&session, right_hand)
                })?;
                let stick_click = timings.measure("openxr/action-state", Duration::ZERO, || {
                    stick_click_action.state(&session, right_hand)
                })?;
                let launcher = timings.measure("openxr/action-state", Duration::ZERO, || {
                    launcher_action.state(&session, right_hand)
                })?;
                let stick_click_down = stick_click.is_active && stick_click.current_state;
                if !stick_click_down {
                    stick_click_consumed = false;
                } else if grabbed_panel.is_some() {
                    stick_click_consumed = true;
                }
                if stick_click.changed_since_last_sync
                    && stick_click_down
                    && let Some(panel_id) = grabbed_panel.take()
                {
                    input.send(XrInput::ClosePanel { panel_id })?;
                    save_dodge_targets_except(&mut panel_frames, &input, Some(panel_id))?;
                }
                let left_click_down =
                    (trigger.is_active && trigger.current_state && resizing_panel.is_none())
                        || (left_face.is_active && left_face.current_state)
                        || (right_face.is_active && right_face.current_state);
                let middle_click_down =
                    stick_click_down && grabbed_panel.is_none() && !stick_click_consumed;
                if launcher.is_active && launcher.current_state && !launcher_pressed {
                    input.send(XrInput::LauncherToggle)?;
                }
                launcher_pressed = launcher.is_active && launcher.current_state;
                for (button, down, previous) in [
                    (0x110, left_click_down, &mut left_click_pressed),
                    (0x112, middle_click_down, &mut middle_click_pressed),
                ] {
                    if down != *previous {
                        input.send(XrInput::Ray {
                            ray: cursor_ray.expect("tracked ray assigned"),
                            gaze_ray,
                            time_ms,
                        })?;
                        input.send(XrInput::Button {
                            button,
                            pressed: down,
                            time_ms,
                        })?;
                        *previous = down;
                    }
                }
            }
        } else {
            cursor_ray = None;
            if grabbed_panel.take().is_some() {
                save_dodge_targets_except(&mut panel_frames, &input, resizing_panel)?;
            }
            finish_resize(
                &mut panel_frames,
                &input,
                &mut resizing_panel,
                resize_geometry,
                resize_edges,
                resize_requested_size,
            )?;
            resize_geometry = None;
            resize_requested_size = None;
            cursor_close_panel = None;
        }
        if !tracked_this_frame {
            cursor_ray = None;
            if grabbed_panel.take().is_some() {
                save_dodge_targets_except(&mut panel_frames, &input, resizing_panel)?;
            }
            finish_resize(
                &mut panel_frames,
                &input,
                &mut resizing_panel,
                resize_geometry,
                resize_edges,
                resize_requested_size,
            )?;
            resize_geometry = None;
            resize_requested_size = None;
            if pointer_tracked {
                input.send(XrInput::PointerLost { time_ms })?;
                left_click_pressed = false;
                middle_click_pressed = false;
                launcher_pressed = false;
            }
        }
        pointer_tracked = tracked_this_frame;
        if !tracked_this_frame && let Some(ray) = gaze_ray {
            let _ = input.try_send(XrInput::GazeRay { ray });
        }
        let active_panel = grabbed_panel.or(resizing_panel);
        if let Some(panel_id) = active_panel
            && active_fullscreen_panel.is_none_or(|fullscreen| fullscreen == panel_id)
        {
            update_dodge_targets(
                &mut panel_frames,
                &[panel_id],
                grab_player_position,
                config.window.collision_margin_m,
            );
        }
        let delta_seconds = (frame_state.predicted_display_period.as_nanos() as f32
            / 1_000_000_000.0)
            .clamp(0.0, 0.1);
        let smoothing = if config.window.animation_half_time_s == 0.0 {
            1.0
        } else {
            1.0 - (-std::f32::consts::LN_2 * delta_seconds / config.window.animation_half_time_s)
                .exp()
        };
        background_exposure += (background_exposure_target - background_exposure) * smoothing;
        if (background_exposure_target - background_exposure).abs() < 0.0001 {
            background_exposure = background_exposure_target;
        }
        for (panel_id, panel) in &mut panel_frames {
            if Some(*panel_id) == active_fullscreen_panel {
                if let Some(anchor) = panel.fullscreen_anchor_pose {
                    let distance = anchor.center.distance(grab_player_position);
                    panel.geometry.pose = PanelGeometry::fit_pose_to_angular_bounds(
                        anchor,
                        panel.geometry.logical_size,
                        distance,
                        config.window.fullscreen_max_width_degrees,
                        config.window.fullscreen_max_height_degrees,
                    );
                }
            } else if Some(*panel_id) == active_panel {
                panel.geometry.pose = panel.temporary_pose;
            } else {
                panel.geometry.pose =
                    smooth_pose(panel.geometry.pose, panel.temporary_pose, smoothing);
            }
        }
        timings.measure("openxr/begin-frame", period, || frame_stream.begin())?;
        if !frame_state.should_render
            || !view_state.contains(
                xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID,
            )
            || views.len() != eyes.len()
            || skybox.is_none()
        {
            timings.measure("openxr/end-frame", period * 2, || {
                frame_stream.end(
                    frame_state.predicted_display_time,
                    xr::EnvironmentBlendMode::OPAQUE,
                    &[],
                )
            })?;
            continue;
        }
        let skybox = skybox.as_mut().expect("skybox is ready for rendering");

        let cursor_scene_pose = match mouse_cursor_state {
            Some(CursorState {
                mouse_controlled: true,
                pose,
            }) => pose,
            _ => cursor_ray.and_then(|ray| {
                cursor_pose(
                    ray,
                    grab_player_position,
                    panel_frames
                        .iter()
                        .filter(|(id, _)| {
                            active_fullscreen_panel.is_none_or(|fullscreen| **id == fullscreen)
                        })
                        .map(|(_, panel)| panel.geometry),
                    &mut cursor_sphere_radius,
                )
            }),
        };
        let panel_draws = panel_frames
            .iter()
            .filter(|(id, _)| active_fullscreen_panel.is_none_or(|fullscreen| **id == fullscreen))
            .map(|(_, panel)| (&panel.texture, panel.geometry))
            .collect::<Vec<_>>();
        let scene_frame = SceneFrame {
            skybox: Some(skybox),
            panels: &panel_draws,
            cursor: cursor_scene_pose,
            cursor_close_panel: cursor_close_panel.and_then(|(id, position)| {
                panel_frames
                    .get(&id)
                    .map(|panel| (panel.geometry, position))
            }),
            grabbed_panel: grabbed_panel
                .and_then(|id| panel_frames.get(&id).map(|panel| panel.geometry)),
            environment_dim: if active_fullscreen_panel.is_some() {
                config.window.fullscreen_environment_dim
            } else {
                0.0
            },
            floor_y,
            texture_sample_phase: rendered_frame_index & 1,
        };
        unsafe {
            timings.measure("gpu/previous-render-wait", period, || {
                device.wait_for_fences(&[fence], true, u64::MAX)
            })?;
            scene.set_background_exposure(background_exposure, &config)?;
            scene.prepare_frame(&scene_frame)?;
            device.reset_fences(&[fence])?;
            device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                command_buffer,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let upload_skybox = skybox.needs_upload();
            if upload_skybox {
                skybox.upload(command_buffer);
            }
            for panel in panel_frames.values() {
                panel.texture.ownership(command_buffer, queue_family, true);
            }
            scene.copy_reflection_textures(command_buffer, &scene_frame);
            for (eye, view) in eyes.iter_mut().zip(&views) {
                let image_index = timings.measure("openxr/acquire-image", period, || {
                    eye.swapchain.acquire_image()
                })?;
                timings.measure("openxr/wait-image", period * 2, || {
                    eye.swapchain.wait_image(xr::Duration::INFINITE)
                })?;
                let target = eye
                    .targets
                    .get(image_index as usize)
                    .context("bad eye swapchain index")?;
                scene.draw(command_buffer, target, view, &scene_frame);
            }
            for panel in panel_frames.values() {
                panel.texture.ownership(command_buffer, queue_family, false);
            }
            device.end_command_buffer(command_buffer)?;
            timings.measure("gpu/queue-submit", Duration::ZERO, || {
                device.queue_submit(
                    queue,
                    &[vk::SubmitInfo::default().command_buffers(&[command_buffer])],
                    fence,
                )
            })?;
            timings.measure("gpu/render-completion", period, || {
                device.wait_for_fences(&[fence], true, u64::MAX)
            })?;
            if upload_skybox {
                skybox.upload_complete();
            }
        }
        for eye in &mut eyes {
            timings.measure("openxr/release-image", Duration::ZERO, || {
                eye.swapchain.release_image()
            })?;
        }

        let projection_views = eyes
            .iter()
            .zip(&views)
            .map(|(eye, view)| {
                xr::CompositionLayerProjectionView::new()
                    .pose(view.pose)
                    .fov(view.fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&eye.swapchain)
                            .image_array_index(0)
                            .image_rect(xr::Rect2Di {
                                offset: xr::Offset2Di { x: 0, y: 0 },
                                extent: xr::Extent2Di {
                                    width: eye.extent.width as i32,
                                    height: eye.extent.height as i32,
                                },
                            }),
                    )
            })
            .collect::<Vec<_>>();
        let projection = xr::CompositionLayerProjection::new()
            .space(&space)
            .views(&projection_views);
        let active_elapsed = active_started.elapsed();
        timings.record_frame(active_elapsed, period);
        timings.record(
            "app/xr-frame-work",
            timings.app_elapsed(active_elapsed),
            period,
        );
        timings.record_external("openxr/frame-calls", "gpu/frame-work", period);
        timings.measure("openxr/end-frame", period * 2, || {
            frame_stream.end(
                frame_state.predicted_display_time,
                xr::EnvironmentBlendMode::OPAQUE,
                &[&projection],
            )
        })?;
        rendered_frame_index = rendered_frame_index.wrapping_add(1);
    }

    unsafe {
        device.device_wait_idle()?;
        panel_frames.clear();
        for eye in &mut eyes {
            eye.targets.clear();
        }
        drop(eyes);
        drop(skybox);
        drop(scene);
        drop((
            aim_space,
            stage_space,
            space,
            frame_waiter,
            frame_stream,
            session,
        ));
        device.destroy_fence(fence, None);
        device.destroy_command_pool(command_pool, None);
        if let Some(pending) = pending_skybox.take() {
            let cleanup_device = device.clone();
            let cleanup_instance = vk_instance.clone();
            thread::spawn(move || {
                if let Ok(Ok(loaded)) = pending.worker.join() {
                    drop(loaded);
                }
                cleanup_device.destroy_device(None);
                cleanup_instance.destroy_instance(None);
            });
        } else {
            device.destroy_device(None);
            vk_instance.destroy_instance(None);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/xr.rs"]
mod color_tests;
