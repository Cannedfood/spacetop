use super::*;

pub(super) struct XrPanel {
    pub texture: PanelTexture,
    pub geometry: PanelGeometry,
    pub saved_pose: PanelPose,
    pub temporary_pose: PanelPose,
    pub resize_pending: bool,
    pub state: PanelState,
    pub fullscreen_anchor_pose: Option<PanelPose>,
    pub history: PanelHistory,
}

pub(super) type PanelImages = std::collections::BTreeMap<PanelId, XrPanel>;

pub(super) fn apply_updates(
    images: &mut PanelImages,
    active_fullscreen_panel: &mut Option<PanelId>,
    pending_spawn: &mut std::collections::BTreeSet<PanelId>,
    maximize_layout_dirty: &mut bool,
    updates: Vec<PanelUpdate>,
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    scene: &SceneRenderer,
    max_panel_size: u32,
    controller: &mut input::ControllerState,
    player: glam::Vec3,
    config: &AppConfig,
    timings: &mut crate::timing::Timings,
) -> Result<()> {
    for update in &updates {
        if let PanelUpdate::Removed { panel_id } = update {
            *maximize_layout_dirty |= images
                .get(panel_id)
                .is_some_and(|panel| panel.state.maximized);
            timings.measure("mixed/panel-retire", Duration::ZERO, || {
                images.remove(panel_id);
            });
            pending_spawn.remove(panel_id);
            if controller.grabbed_panel == Some(*panel_id) {
                controller.grabbed_panel = None;
            }
            if *active_fullscreen_panel == Some(*panel_id) {
                *active_fullscreen_panel = images
                    .iter()
                    .rev()
                    .find(|(_, panel)| panel.state.fullscreen)
                    .map(|(id, _)| *id);
                if active_fullscreen_panel.is_none() {
                    for panel in images.values_mut() {
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
                state,
            } => {
                let shared = timings
                    .measure("gpu/dmabuf-import", Duration::ZERO, || {
                        SharedImage::import(instance, device, physical_device, dmabuf)
                    })
                    .context("mandatory GPU DMA-BUF import failed")?;
                let size = shared.dmabuf.size();
                ensure!(
                    size.w > 0
                        && size.h > 0
                        && size.w as u32 <= max_panel_size
                        && size.h as u32 <= max_panel_size,
                    "window image exceeds negotiated Vulkan limits"
                );
                let texture = timings.measure("gpu/panel-texture", Duration::ZERO, || {
                    PanelTexture::new(scene, shared)
                })?;
                timings.measure("gpu/panel-replace", Duration::ZERO, || {
                    if let Some(panel) = images.get_mut(&panel_id) {
                        let fullscreen_changed = panel.state.fullscreen != state.fullscreen;
                        let maximized_changed = panel.state.maximized != state.maximized;
                        let layout_changed =
                            fullscreen_changed || (!state.fullscreen && maximized_changed);
                        if layout_changed {
                            panel.history.save(
                                panel.state,
                                PanelPastState {
                                    distance: panel.geometry.pose.center.distance(player),
                                    size: panel.geometry.logical_size,
                                },
                            );
                        }
                        reconcile_panel_geometry(
                            &mut panel.geometry,
                            &mut panel.saved_pose,
                            &mut panel.temporary_pose,
                            geometry,
                            panel.resize_pending && panel.state.fullscreen == state.fullscreen,
                            false,
                        );
                        if layout_changed {
                            let distance = panel
                                .history
                                .get(state)
                                .map(|past| past.distance)
                                .unwrap_or_else(|| panel.geometry.pose.center.distance(player));
                            let pose = restore_panel_distance(
                                panel.geometry.pose,
                                player,
                                distance,
                                geometry.logical_size,
                                config.window.pixels_per_degree,
                            );
                            panel.geometry.pose = pose;
                            panel.saved_pose = pose;
                            panel.temporary_pose = pose;
                            panel.resize_pending = false;
                            reset_grab_baseline_after_unmaximize(
                                controller.grabbed_panel,
                                panel_id,
                                pose.width_m,
                                distance,
                                &mut controller.grab_initial_width,
                                &mut controller.grab_initial_radius,
                            );
                            if controller.grabbed_panel == Some(panel_id) {
                                controller.grab_radius = distance;
                            }
                        }
                        panel.texture = texture;
                        panel.state = state;
                        if maximized_changed {
                            *maximize_layout_dirty = true;
                            if state.maximized {
                                controller.resizing_panel = None;
                                controller.resize_density = None;
                                controller.resize_requested_size = None;
                            }
                        }
                        if fullscreen_changed {
                            panel.fullscreen_anchor_pose = None;
                            if state.fullscreen {
                                *active_fullscreen_panel = Some(panel_id);
                            } else if *active_fullscreen_panel == Some(panel_id) {
                                *active_fullscreen_panel = images
                                    .iter()
                                    .rev()
                                    .find(|(_, candidate)| candidate.state.fullscreen)
                                    .map(|(id, _)| *id);
                                if active_fullscreen_panel.is_none() {
                                    for panel in images.values_mut() {
                                        panel.geometry.pose = panel.saved_pose;
                                        panel.temporary_pose = panel.saved_pose;
                                    }
                                }
                            }
                            if state.fullscreen {
                                controller.grabbed_panel = None;
                                controller.resizing_panel = None;
                                controller.resize_density = None;
                                controller.resize_requested_size = None;
                            }
                        }
                    } else {
                        images.insert(
                            panel_id,
                            XrPanel {
                                texture,
                                geometry,
                                saved_pose: geometry.pose,
                                temporary_pose: geometry.pose,
                                resize_pending: false,
                                state,
                                fullscreen_anchor_pose: None,
                                history: PanelHistory::default(),
                            },
                        );
                        if state.fullscreen {
                            *active_fullscreen_panel = Some(panel_id);
                        }
                        pending_spawn.insert(panel_id);
                        if state.maximized {
                            *maximize_layout_dirty = true;
                        }
                    }
                });
            }
            PanelUpdate::Removed { .. } => {}
        }
    }
    Ok(())
}

pub(super) fn reconcile_panel_geometry(
    current: &mut PanelGeometry,
    saved_pose: &mut PanelPose,
    temporary_pose: &mut PanelPose,
    committed: PanelGeometry,
    resizing: bool,
    preserve_saved_width: bool,
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
    if !preserve_saved_width {
        saved_pose.width_m = committed.pose.width_m;
        temporary_pose.width_m = committed.pose.width_m;
    }
}

pub(super) fn update_dodge_targets(
    panels: &mut PanelImages,
    fixed_windows: &[PanelId],
    player: glam::Vec3,
    margin_m: f32,
) {
    if panels.values().any(|panel| panel.state.fullscreen) {
        return;
    }
    let geometries: Vec<_> = panels
        .iter()
        .map(|(id, panel)| {
            let pose = if fixed_windows.contains(id) || panel.state.maximized {
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

pub(super) fn reset_grab_baseline_after_unmaximize(
    grabbed_panel: Option<PanelId>,
    panel_id: PanelId,
    restored_width: f32,
    grab_radius: f32,
    grab_initial_width: &mut f32,
    grab_initial_radius: &mut f32,
) {
    if grabbed_panel == Some(panel_id) {
        *grab_initial_width = restored_width;
        *grab_initial_radius = grab_radius;
    }
}

pub(super) fn save_dodge_targets(
    panels: &mut PanelImages,
    input: &crate::bridge::InputSender,
) -> Result<()> {
    save_dodge_targets_except(panels, input, None)
}

pub(super) fn save_dodge_targets_except(
    panels: &mut PanelImages,
    input: &crate::bridge::InputSender,
    excluded_panel: Option<PanelId>,
) -> Result<()> {
    if panels.values().any(|panel| panel.state.fullscreen) {
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

pub(super) fn finish_resize(
    panels: &mut PanelImages,
    input: &crate::bridge::InputSender,
    resizing_panel: &mut Option<PanelId>,
    density: Option<f32>,
    requested_size: Option<(i32, i32)>,
) -> Result<()> {
    if let Some(panel_id) = resizing_panel.take() {
        if let Some((density, (width, height))) = density.zip(requested_size) {
            input.send(XrInput::ResizePanel {
                panel_id,
                width,
                height,
                pixels_per_degree: Some(density),
            })?;
        }
        save_dodge_targets_except(panels, input, Some(panel_id))?;
    }
    Ok(())
}

pub(super) fn smooth_pose(current: PanelPose, target: PanelPose, factor: f32) -> PanelPose {
    let angle_delta = |from: f32, to: f32| PanelPose::wrap_angle(to - from);
    PanelPose {
        center: current.center.lerp(target.center, factor),
        yaw: current.yaw + angle_delta(current.yaw, target.yaw) * factor,
        pitch: current.pitch + (target.pitch - current.pitch) * factor,
        width_m: current.width_m + (target.width_m - current.width_m) * factor,
    }
}

pub(super) fn restore_panel_distance(
    current: PanelPose,
    player: glam::Vec3,
    distance: f32,
    size: smithay::utils::Size<i32, smithay::utils::Logical>,
    density: f32,
) -> PanelPose {
    let offset = current.center - player;
    let direction = offset.try_normalize().unwrap_or(glam::Vec3::NEG_Z);
    PanelPose {
        width_m: PanelPose::width_for_pixel_density(size.w.max(1) as f32, distance, density),
        ..PanelPose::looking_from_to(player + direction * distance, player)
    }
}
