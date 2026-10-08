//! OpenXR application client. This connects to the configured system runtime;
//! it does not implement an OpenXR runtime or provide inter-application overlay
//! semantics.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::bridge::{CursorState, PanelReceiver, PanelState, PanelUpdate, XrInput};
use crate::config::{AppConfig, ConfigWatcher};
use crate::gpu::{self, SharedImage};
use crate::panel::{
    PanelGeometry, PanelHistory, PanelId, PanelPastState, PanelPose, Ray3, dodge_windows,
};
use crate::scene::{PanelTexture, SceneFrame, SceneRenderer, SkyboxTexture};
use anyhow::{Context, Result, ensure};
use ash::vk::{self, Handle};
use openxr as xr;
use smithay::backend::allocator::Buffer;

mod actions;
mod background;
mod graphics;
mod input;
mod panels;

use graphics::XrEye;
#[cfg(test)]
use graphics::panel_swapchain_format;
use input::cursor_pose;
use panels::{
    PanelImages, finish_resize, save_dodge_targets, save_dodge_targets_except, smooth_pose,
    update_dodge_targets,
};
#[cfg(test)]
use panels::{
    reconcile_panel_geometry, reset_grab_baseline_after_unmaximize, restore_panel_distance,
};

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

/// Run the OpenXR client loop until the runtime requests session/application exit.
///
/// It renders captured Wayland panels into a Vulkan stereo projection layer and
/// forwards the right-hand aim/select controller actions to the compositor.
pub fn run(
    frames: PanelReceiver,
    input: crate::bridge::InputSender,
    mut config: AppConfig,
) -> Result<()> {
    let config_watcher = ConfigWatcher::new_default()?;
    let graphics::Graphics {
        xr_entry: _xr_entry,
        vk_entry: _vk_entry,
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
        mut frame_waiter,
        mut frame_stream,
        device_limit,
        max_panel_size,
        format,
    } = graphics::Graphics::new()?;
    let mut scene = SceneRenderer::new(&device, &vk_instance, physical_device, format, &config)?;
    let mut background =
        background::Background::new(&mut scene, &vk_instance, physical_device, &config)?;
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
            max_panel_size,
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
    let actions = actions::Actions::new(&instance, &session)?;
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut exit = false;
    let mut panel_frames = PanelImages::new();
    let mut active_fullscreen_panel = None;
    let mut pending_spawn = std::collections::BTreeSet::new();
    let mut controller = input::ControllerState::new(config.window.default_distance_m);
    let mut cursor_sphere_radius = config.window.default_distance_m;
    let mut grab_player_position = glam::Vec3::ZERO;
    let mut maximize_layout_dirty = false;
    let mut timings = crate::timing::Timings::new();
    let mut rendered_frame_index = 0_u32;
    let mut mouse_cursor_state: Option<CursorState> = None;

    while !exit {
        background.poll(&mut scene, &vk_instance, physical_device, &config)?;
        if let Some(reload) = config_watcher.take_reload() {
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
                                background.reload(image, &scene, &vk_instance, physical_device);
                            }
                            if stage_space.is_none() || !stage_floor_calibrated {
                                floor_y = next_config.floor.height_m;
                            }
                            cursor_sphere_radius = next_config.window.default_distance_m;
                            if controller.grabbed_panel.is_none() {
                                controller.grab_radius = next_config.window.default_distance_m;
                                controller.grab_initial_radius =
                                    next_config.window.default_distance_m;
                            }
                            if next_config.window != config.window
                                && let Err(error) = input.send(XrInput::ConfigReloaded {
                                    window: Box::new(next_config.window.clone()),
                                })
                            {
                                eprintln!("failed to notify compositor of config reload: {error}");
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
                            controller.reset_tracking();
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
        panels::PanelUpdates {
            images: &mut panel_frames,
            active_fullscreen_panel: &mut active_fullscreen_panel,
            pending_spawn: &mut pending_spawn,
            maximize_layout_dirty: &mut maximize_layout_dirty,
        }
        .apply(
            updates,
            panels::PanelImport {
                instance: &vk_instance,
                device: &device,
                physical_device,
                scene: &scene,
                max_panel_size,
            },
            &mut controller,
            grab_player_position,
            &config,
            &mut timings,
        )?;
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
            for panel in panel_frames
                .values_mut()
                .filter(|panel| panel.state.maximized && !panel.state.fullscreen)
            {
                let distance = panel.geometry.pose.center.distance(grab_player_position);
                panel.geometry.pose = PanelGeometry::fit_pose_to_angular_bounds(
                    panel.geometry.pose,
                    panel.geometry.logical_size,
                    distance,
                    config.window.maximized_max_width_degrees,
                    config.window.maximized_max_height_degrees,
                );
            }
            if maximize_layout_dirty && active_fullscreen_panel.is_none() {
                let fixed_panel = controller.grabbed_panel.or(controller.resizing_panel);
                let fixed_windows: Vec<_> = fixed_panel.into_iter().collect();
                update_dodge_targets(
                    &mut panel_frames,
                    &fixed_windows,
                    grab_player_position,
                    config.window.collision_margin_m,
                );
                if fixed_panel.is_none() {
                    save_dodge_targets(&mut panel_frames, &input)?;
                }
                maximize_layout_dirty = false;
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
        controller.update(
            &actions,
            input::ControllerFrame {
                session: &session,
                space: &space,
                frame_state: &frame_state,
                gaze_ray,
                player: grab_player_position,
                active_fullscreen_panel,
                config: &config,
                input: &input,
                timings: &mut timings,
                panels: &mut panel_frames,
            },
        )?;
        let active_panel = controller.grabbed_panel.or(controller.resizing_panel);
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
        background.animate(smoothing);
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
            if panel.state.maximized && Some(*panel_id) != active_fullscreen_panel {
                let distance = panel.geometry.pose.center.distance(grab_player_position);
                panel.geometry.pose = PanelGeometry::fit_pose_to_angular_bounds(
                    panel.geometry.pose,
                    panel.geometry.logical_size,
                    distance,
                    config.window.maximized_max_width_degrees,
                    config.window.maximized_max_height_degrees,
                );
            }
        }
        timings.measure("openxr/begin-frame", period, || frame_stream.begin())?;
        if !frame_state.should_render
            || !view_state.contains(
                xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID,
            )
            || views.len() != eyes.len()
            || background.skybox.is_none()
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
        let skybox = background
            .skybox
            .as_mut()
            .expect("skybox is ready for rendering");

        let _ = input.try_send(XrInput::PresentedPanels {
            geometries: panel_frames
                .iter()
                .filter(|(id, _)| {
                    active_fullscreen_panel.is_none_or(|fullscreen| **id == fullscreen)
                })
                .map(|(id, panel)| (*id, panel.geometry))
                .collect(),
        });

        let cursor_scene_pose = match mouse_cursor_state {
            Some(CursorState {
                mouse_controlled: true,
                pose,
            }) => pose,
            _ => controller.cursor_ray.and_then(|ray| {
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
            cursor_close_panel: controller.cursor_close_panel.and_then(|(id, position)| {
                panel_frames
                    .get(&id)
                    .map(|panel| (panel.geometry, position))
            }),
            grabbed_panel: controller
                .grabbed_panel
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
            scene.set_background_exposure(background.exposure, &config)?;
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
        drop(background.skybox);
        drop(scene);
        drop((
            actions.aim_space,
            stage_space,
            space,
            frame_waiter,
            frame_stream,
            session,
        ));
        device.destroy_fence(fence, None);
        device.destroy_command_pool(command_pool, None);
        if let Some(pending) = background.pending.take() {
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
