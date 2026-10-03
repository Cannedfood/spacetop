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
use anyhow::{Context, Result, ensure};
use ash::{
    Entry as VkEntry,
    vk::{self, Handle},
};
use openxr as xr;
use smithay::backend::allocator::Buffer;

struct XrPanel {
    shared: SharedImage,
    geometry: PanelGeometry,
    swapchain: xr::Swapchain<xr::Vulkan>,
    images: Vec<u64>,
}

impl XrPanel {
    fn new(
        session: &xr::Session<xr::Vulkan>,
        format: vk::Format,
        shared: SharedImage,
        geometry: PanelGeometry,
        timings: &mut crate::timing::Timings,
    ) -> Result<Self> {
        let size = shared.dmabuf.size();
        let swapchain = timings
            .measure("openxr/create-swapchain", Duration::ZERO, || {
                session.create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    usage_flags: xr::SwapchainUsageFlags::TRANSFER_DST
                        | xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
                    format: format.as_raw() as _,
                    sample_count: 1,
                    width: size.w as u32,
                    height: size.h as u32,
                    face_count: 1,
                    array_size: 1,
                    mip_count: 1,
                })
            })
            .context("create window swapchain")?;
        let images = timings.measure("openxr/enumerate-images", Duration::ZERO, || {
            swapchain.enumerate_images()
        })?;
        Ok(Self {
            shared,
            geometry,
            swapchain,
            images,
        })
    }
}

type PanelImages = std::collections::BTreeMap<u64, XrPanel>;

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

fn cursor_pose(
    ray: Ray3,
    player: glam::Vec3,
    panels: impl Iterator<Item = PanelGeometry>,
) -> Option<PanelPose> {
    let direction = ray.direction.try_normalize()?;
    let ray = Ray3 { direction, ..ray };
    let nearest = panels
        .filter_map(|geometry| geometry.intersect(ray).map(|hit| (geometry.pose, hit)))
        .min_by(|(_, first), (_, second)| first.distance_m.total_cmp(&second.distance_m));
    if let Some((pose, hit)) = nearest {
        return Some(PanelPose {
            center: ray.origin + direction * hit.distance_m,
            width_m: 0.021,
            ..pose
        });
    }
    let radius = PanelPose::for_slot(0).center.length();
    let offset = ray.origin - player;
    let projection = offset.dot(direction);
    let discriminant = projection * projection - offset.length_squared() + radius * radius;
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
/// It submits captured Wayland panel pixels as independent quad layers and
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
        max_width: graphics
            .max_swapchain_image_width
            .min(device_limit)
            .min(cap)
            .min(i32::MAX as u32),
        max_height: graphics
            .max_swapchain_image_height
            .min(device_limit)
            .min(cap)
            .min(i32::MAX as u32),
        max_layers: graphics.max_layer_count.saturating_sub(1),
    };
    ensure!(
        limits.max_width > 0 && limits.max_height > 0 && limits.max_layers > 0,
        "invalid OpenXR panel limits, no layer available beside the cursor, or zero SPACETOP_MAX_PANEL_SIZE"
    );
    eprintln!(
        "OpenXR panel limits: {}x{}, {} window layers",
        limits.max_width, limits.max_height, limits.max_layers
    );
    let formats = session.enumerate_swapchain_formats()?;
    let format = panel_swapchain_format(&formats)?;
    let render_node = gpu::render_node(&vk_instance, physical_device)
        .context("GPU sharing requires a DRM render node for the runtime-selected Vulkan GPU")?;
    input
        .send(XrInput::GpuDevice {
            render_node,
            limits,
        })
        .context("request GPU compositing")?;
    let gpu_cursor = gpu::GpuCursor::new(&vk_instance, &device, physical_device)?;
    ensure!(
        graphics.max_swapchain_image_width >= 21
            && graphics.max_swapchain_image_height >= 21
            && device_limit >= 21,
        "OpenXR cannot accommodate the cursor image"
    );
    let mut cursor_swapchain = session
        .create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::TRANSFER_DST
                | xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
            format: format.as_raw() as _,
            sample_count: 1,
            width: 21,
            height: 21,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        })
        .context("create cursor swapchain")?;
    let cursor_images = cursor_swapchain.enumerate_images()?;
    let space =
        session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
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
                    let size = dmabuf.size();
                    ensure!(
                        size.w > 0
                            && size.h > 0
                            && size.w as u32 <= limits.max_width
                            && size.h as u32 <= limits.max_height,
                        "window image exceeds negotiated OpenXR limits"
                    );
                    let shared = timings
                        .measure("gpu/dmabuf-import", Duration::ZERO, || {
                            SharedImage::import(&vk_instance, &device, physical_device, dmabuf)
                        })
                        .context("mandatory GPU DMA-BUF import failed")?;
                    if let Some(panel) = panel_frames.get_mut(&panel_id)
                        && panel.shared.dmabuf.size() == size
                    {
                        timings.measure("gpu/panel-replace", Duration::ZERO, || {
                            panel.shared = shared;
                            panel.geometry = geometry;
                        });
                    } else {
                        ensure!(
                            panel_frames.contains_key(&panel_id)
                                || panel_frames.len() < limits.max_layers as usize,
                            "OpenXR supports at most {} mapped window layers",
                            limits.max_layers
                        );
                        let panel = XrPanel::new(&session, format, shared, geometry, &mut timings)?;
                        timings.measure("mixed/panel-replace", Duration::ZERO, || {
                            panel_frames.insert(panel_id, panel);
                        });
                    }
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
        if !frame_state.should_render {
            timings.measure("openxr/end-frame", period * 2, || {
                frame_stream.end(
                    frame_state.predicted_display_time,
                    xr::EnvironmentBlendMode::OPAQUE,
                    &[],
                )
            })?;
            continue;
        }

        let cursor_quad_pose = cursor_ray.and_then(|ray| {
            cursor_pose(
                ray,
                grab_player_position,
                panel_frames.values().map(|panel| panel.geometry),
            )
        });
        unsafe {
            timings.measure("gpu/previous-copy-wait", period, || {
                device.wait_for_fences(&[fence], true, u64::MAX)
            })?;
            device.reset_fences(&[fence])?;
            device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                command_buffer,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1);
            for panel in panel_frames.values_mut() {
                let image_index = timings.measure("openxr/acquire-image", period, || {
                    panel.swapchain.acquire_image()
                })?;
                timings.measure("openxr/wait-image", period * 2, || {
                    panel.swapchain.wait_image(xr::Duration::INFINITE)
                })?;
                let image = vk::Image::from_raw(
                    *panel
                        .images
                        .get(image_index as usize)
                        .context("bad window swapchain index")? as _,
                );
                let begin_barrier = vk::ImageMemoryBarrier::default()
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range);
                device.cmd_pipeline_barrier(
                    command_buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[begin_barrier],
                );
                panel
                    .shared
                    .copy_to(&device, command_buffer, image, queue_family);
                let end_barrier = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(
                        vk::AccessFlags::COLOR_ATTACHMENT_READ
                            | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    )
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range);
                device.cmd_pipeline_barrier(
                    command_buffer,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[end_barrier],
                );
            }
            if cursor_quad_pose.is_some() {
                let image_index = timings.measure("openxr/acquire-image", period, || {
                    cursor_swapchain.acquire_image()
                })?;
                timings.measure("openxr/wait-image", period * 2, || {
                    cursor_swapchain.wait_image(xr::Duration::INFINITE)
                })?;
                let image = vk::Image::from_raw(
                    *cursor_images
                        .get(image_index as usize)
                        .context("bad cursor swapchain index")? as _,
                );
                let begin_barrier = vk::ImageMemoryBarrier::default()
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range);
                device.cmd_pipeline_barrier(
                    command_buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[begin_barrier],
                );
                gpu_cursor.draw_quad(command_buffer, image);
                let end_barrier = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(
                        vk::AccessFlags::COLOR_ATTACHMENT_READ
                            | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    )
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range);
                device.cmd_pipeline_barrier(
                    command_buffer,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[end_barrier],
                );
            }
            device.end_command_buffer(command_buffer)?;
            timings.measure("gpu/queue-submit", Duration::ZERO, || {
                device.queue_submit(
                    queue,
                    &[vk::SubmitInfo::default().command_buffers(&[command_buffer])],
                    fence,
                )
            })?;
            timings.measure("gpu/copy-completion", period, || {
                device.wait_for_fences(&[fence], true, u64::MAX)
            })?;
        }
        for panel in panel_frames.values_mut() {
            timings.measure("openxr/release-image", Duration::ZERO, || {
                panel.swapchain.release_image()
            })?;
        }
        if cursor_quad_pose.is_some() {
            timings.measure("openxr/release-image", Duration::ZERO, || {
                cursor_swapchain.release_image()
            })?;
        }

        let quads = panel_frames
            .values()
            .map(|panel| {
                let pose = panel.geometry.pose;
                let frame = &panel.shared;
                let orientation = pose.orientation();
                let size = xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di {
                        width: frame.dmabuf.size().w,
                        height: frame.dmabuf.size().h,
                    },
                };
                xr::CompositionLayerQuad::new()
                    .space(&space)
                    .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&panel.swapchain)
                            .image_array_index(0)
                            .image_rect(size),
                    )
                    .pose(xr::Posef {
                        orientation: xr::Quaternionf {
                            x: orientation.x,
                            y: orientation.y,
                            z: orientation.z,
                            w: orientation.w,
                        },
                        position: xr::Vector3f {
                            x: pose.center.x,
                            y: pose.center.y,
                            z: pose.center.z,
                        },
                    })
                    .size(xr::Extent2Df {
                        width: pose.width_m,
                        height: pose.width_m * panel.geometry.logical_size.h as f32
                            / panel.geometry.logical_size.w.max(1) as f32,
                    })
            })
            .collect::<Vec<_>>();
        let cursor_quad = cursor_quad_pose.map(|pose| {
            let orientation = pose.orientation();
            xr::CompositionLayerQuad::new()
                .space(&space)
                .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
                .sub_image(
                    xr::SwapchainSubImage::new()
                        .swapchain(&cursor_swapchain)
                        .image_array_index(0)
                        .image_rect(xr::Rect2Di {
                            offset: xr::Offset2Di { x: 0, y: 0 },
                            extent: xr::Extent2Di {
                                width: 21,
                                height: 21,
                            },
                        }),
                )
                .pose(xr::Posef {
                    orientation: xr::Quaternionf {
                        x: orientation.x,
                        y: orientation.y,
                        z: orientation.z,
                        w: orientation.w,
                    },
                    position: xr::Vector3f {
                        x: pose.center.x,
                        y: pose.center.y,
                        z: pose.center.z,
                    },
                })
                .size(xr::Extent2Df {
                    width: pose.width_m,
                    height: pose.width_m,
                })
        });
        let layers = quads
            .iter()
            .chain(cursor_quad.iter())
            .map(|quad| quad as &xr::CompositionLayerBase<xr::Vulkan>)
            .collect::<Vec<_>>();
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
                &layers,
            )
        })?;
    }

    unsafe {
        device.device_wait_idle()?;
        panel_frames.clear();
        drop(cursor_swapchain);
        drop((aim_space, space, frame_waiter, frame_stream, session));
        drop(gpu_cursor);
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
