//! OpenXR application client. This connects to the configured system runtime;
//! it does not implement an OpenXR runtime or provide inter-application overlay
//! semantics.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::bridge::{PanelReceiver, PanelUpdate, XrInput};
use crate::gpu::{self, SharedImage};
use crate::panel::{PanelGeometry, PanelLimits, PanelPose, Ray3};
use crate::scene::{
    FALLBACK_FLOOR_Y, PanelTexture, RenderTarget, SceneFrame, SceneRenderer, SkyboxTexture,
};
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
}

struct XrEye {
    swapchain: xr::Swapchain<xr::Vulkan>,
    targets: Vec<RenderTarget>,
    extent: vk::Extent2D,
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
        ..PanelPose::facing_player(ray.origin + direction * distance, player)
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
pub fn run(frames: PanelReceiver, input: crate::bridge::InputSender) -> Result<()> {
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
    if loader_version >= vk::API_VERSION_1_1
        && requirements.max_api_version_supported >= xr::Version::new(1, 1, 0)
    {
        vk_api_version = vk_api_version.max(vk::API_VERSION_1_1);
    }
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
    let scene = SceneRenderer::new(&device, format)?;
    let mut skybox = SkyboxTexture::new(&scene, &vk_instance, physical_device)?;
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
                eprintln!("OpenXR STAGE floor unavailable: {error}; using fallback floor height");
                None
            }
        }
    } else {
        eprintln!("OpenXR STAGE unsupported; using fallback floor height {FALLBACK_FLOOR_Y}m");
        None
    };
    let mut floor_y = FALLBACK_FLOOR_Y;
    let action_set = instance.create_action_set("spacetop", "Spacetop input", 0)?;
    let right_hand = instance.string_to_path("/user/hand/right")?;
    let aim_action =
        action_set.create_action::<xr::Posef>("aim_pose", "Aim pose", &[right_hand])?;
    let trigger_action = action_set.create_action::<bool>("trigger", "Trigger", &[right_hand])?;
    let secondary_action =
        action_set.create_action::<bool>("secondary", "Secondary click", &[right_hand])?;
    let grip_action = action_set.create_action::<bool>("grip", "Grip", &[right_hand])?;
    let stick_action =
        action_set.create_action::<xr::Vector2f>("stick", "Thumbstick", &[right_hand])?;
    let aim_path = instance.string_to_path("/user/hand/right/input/aim/pose")?;
    for (profile, button, secondary, grip, stick) in [
        (
            "/interaction_profiles/khr/simple_controller",
            "select/click",
            None,
            None,
            None,
        ),
        (
            "/interaction_profiles/oculus/touch_controller",
            "trigger/value",
            Some("b/click"),
            Some("squeeze/value"),
            Some("thumbstick"),
        ),
        (
            "/interaction_profiles/valve/index_controller",
            "trigger/click",
            Some("b/click"),
            Some("squeeze/value"),
            Some("thumbstick"),
        ),
        (
            "/interaction_profiles/htc/vive_controller",
            "trigger/click",
            Some("trackpad/click"),
            Some("squeeze/click"),
            Some("trackpad"),
        ),
        (
            "/interaction_profiles/microsoft/motion_controller",
            "trigger/value",
            Some("trackpad/click"),
            Some("squeeze/click"),
            Some("thumbstick"),
        ),
    ] {
        let mut bindings = vec![
            xr::Binding::new(&aim_action, aim_path),
            xr::Binding::new(
                &trigger_action,
                instance.string_to_path(&format!("/user/hand/right/input/{button}"))?,
            ),
        ];
        if let Some(secondary) = secondary {
            bindings.push(xr::Binding::new(
                &secondary_action,
                instance.string_to_path(&format!("/user/hand/right/input/{secondary}"))?,
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
        instance
            .suggest_interaction_profile_bindings(instance.string_to_path(profile)?, &bindings)?;
    }
    session.attach_action_sets(&[&action_set])?;
    let aim_space = aim_action.create_space(&session, right_hand, xr::Posef::IDENTITY)?;
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut exit = false;
    let mut panel_frames = PanelImages::new();
    let mut cursor_ray: Option<Ray3> = None;
    let mut cursor_sphere_radius = PanelPose::for_slot(0).center.length();
    let mut pointer_tracked = false;
    let mut trigger_pressed = false;
    let mut secondary_pressed = false;
    let mut grabbed_panel: Option<u64> = None;
    let mut grab_radius = 1.6_f32;
    let mut grab_player_position = glam::Vec3::ZERO;
    let mut grab_initial_radius = 1.6_f32;
    let mut grab_initial_width = 1.0_f32;
    let mut grab_direction_offset = glam::Vec2::ZERO;
    let mut timings = crate::timing::Timings::new();

    while !exit {
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
                            trigger_pressed = false;
                            secondary_pressed = false;
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
        let updates = frames.drain();
        for update in &updates {
            if let PanelUpdate::Removed { panel_id } = update {
                timings.measure("mixed/panel-retire", Duration::ZERO, || {
                    panel_frames.remove(panel_id);
                });
                if grabbed_panel == Some(*panel_id) {
                    grabbed_panel = None;
                }
            }
        }
        for command in updates {
            match command {
                PanelUpdate::GpuFrame {
                    panel_id,
                    dmabuf,
                    geometry,
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
                        panel_frames.insert(panel_id, XrPanel { texture, geometry });
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
            floor_y = tracked_floor_height(floor_y, location);
        }
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
        }
        // Update the action set and forward the right controller's aim ray.
        timings.measure("openxr/sync-actions", Duration::ZERO, || {
            session.sync_actions(&[xr::ActiveActionSet::new(&action_set)])
        })?;
        let mut tracked_this_frame = false;
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
                let time_ms = (frame_state.predicted_display_time.as_nanos() / 1_000_000) as u32;
                let _ = input.try_send(XrInput::Ray {
                    ray: cursor_ray.expect("ray assigned above"),
                    time_ms,
                });
                let pointing_at_window = cursor_ray.is_some_and(|ray| {
                    panel_frames
                        .values()
                        .any(|panel| panel.geometry.intersect(ray).is_some())
                });
                let grip = timings.measure("openxr/action-state", Duration::ZERO, || {
                    grip_action.state(&session, right_hand)
                })?;
                if grip.changed_since_last_sync {
                    if grip.current_state {
                        grabbed_panel = cursor_ray.and_then(|ray| {
                            panel_frames
                                .iter()
                                .filter_map(|(id, panel)| {
                                    panel.geometry.intersect(ray).map(|hit| (*id, hit))
                                })
                                .min_by(|(_, first), (_, second)| {
                                    first.distance_m.total_cmp(&second.distance_m)
                                })
                                .map(|(id, _hit)| {
                                    let panel = &panel_frames[&id];
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
                        grabbed_panel = None;
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
                let trigger = timings.measure("openxr/action-state", Duration::ZERO, || {
                    trigger_action.state(&session, right_hand)
                })?;
                let secondary = timings.measure("openxr/action-state", Duration::ZERO, || {
                    secondary_action.state(&session, right_hand)
                })?;
                for (button, down, previous) in [
                    (
                        0x110,
                        trigger.is_active && trigger.current_state,
                        &mut trigger_pressed,
                    ),
                    (
                        0x111,
                        secondary.is_active && secondary.current_state,
                        &mut secondary_pressed,
                    ),
                ] {
                    if down != *previous {
                        input.send(XrInput::Ray {
                            ray: cursor_ray.expect("tracked ray assigned"),
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
            grabbed_panel = None;
        }
        if !tracked_this_frame {
            cursor_ray = None;
            grabbed_panel = None;
            if pointer_tracked {
                input.send(XrInput::PointerLost {
                    time_ms: (frame_state.predicted_display_time.as_nanos() / 1_000_000) as u32,
                })?;
                trigger_pressed = false;
                secondary_pressed = false;
            }
        }
        pointer_tracked = tracked_this_frame;
        timings.measure("openxr/begin-frame", period, || frame_stream.begin())?;
        if !frame_state.should_render
            || !view_state.contains(
                xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID,
            )
            || views.len() != eyes.len()
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

        let cursor_scene_pose = cursor_ray.and_then(|ray| {
            cursor_pose(
                ray,
                grab_player_position,
                panel_frames.values().map(|panel| panel.geometry),
                &mut cursor_sphere_radius,
            )
        });
        let panel_draws = panel_frames
            .values()
            .map(|panel| (&panel.texture, panel.geometry))
            .collect::<Vec<_>>();
        let scene_frame = SceneFrame {
            skybox: Some(&skybox),
            panels: &panel_draws,
            cursor: cursor_scene_pose,
            floor_y,
        };
        unsafe {
            timings.measure("gpu/previous-render-wait", period, || {
                device.wait_for_fences(&[fence], true, u64::MAX)
            })?;
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
    }

    unsafe {
        device.device_wait_idle()?;
        panel_frames.clear();
        for eye in &mut eyes {
            eye.targets.clear();
        }
        drop(eyes);
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
        device.destroy_device(None);
        vk_instance.destroy_instance(None);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/xr.rs"]
mod color_tests;
