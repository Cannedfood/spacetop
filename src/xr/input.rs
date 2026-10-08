use super::*;
use actions::Actions;

pub(super) fn cursor_pose(
    ray: Ray3,
    player: glam::Vec3,
    panels: impl Iterator<Item = PanelGeometry>,
    sphere_radius: &mut f32,
) -> Option<PanelPose> {
    let direction = ray.direction.try_normalize()?;
    let ray = Ray3 { direction, ..ray };
    let nearest = panels
        .filter_map(|geometry| {
            geometry
                .trace(ray, 0.0)
                .filter(|hit| hit.on_content)
                .map(|hit| (geometry.pose, hit))
        })
        .min_by(|(_, first), (_, second)| first.distance_m.total_cmp(&second.distance_m));
    if let Some((pose, hit)) = nearest {
        let center = hit.position;
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

pub(super) struct ControllerState {
    pub cursor_ray: Option<Ray3>,
    pub grabbed_panel: Option<PanelId>,
    pub grab_radius: f32,
    pub grab_initial_radius: f32,
    pub grab_initial_width: f32,
    pub resizing_panel: Option<PanelId>,
    pub resize_density: Option<f32>,
    pub resize_requested_size: Option<(i32, i32)>,
    pub cursor_close_panel: Option<(PanelId, glam::Vec2)>,
    pointer_tracked: bool,
    left_click_pressed: bool,
    middle_click_pressed: bool,
    stick_click_consumed: bool,
    launcher_pressed: bool,
    maximize_right_chord_pressed: bool,
    grab_direction_offset: glam::Vec2,
    resize_previous_ray: Option<Ray3>,
    resize_size: glam::Vec2,
    resize_edges: [bool; 4],
}

impl ControllerState {
    pub(super) fn new(distance: f32) -> Self {
        Self {
            cursor_ray: None,
            grabbed_panel: None,
            grab_radius: distance,
            grab_initial_radius: distance,
            grab_initial_width: 1.0,
            resizing_panel: None,
            resize_density: None,
            resize_requested_size: None,
            cursor_close_panel: None,
            pointer_tracked: false,
            left_click_pressed: false,
            middle_click_pressed: false,
            stick_click_consumed: false,
            launcher_pressed: false,
            maximize_right_chord_pressed: false,
            grab_direction_offset: glam::Vec2::ZERO,
            resize_previous_ray: None,
            resize_size: glam::Vec2::ZERO,
            resize_edges: [false; 4],
        }
    }

    pub(super) fn reset_tracking(&mut self) {
        self.pointer_tracked = false;
        self.left_click_pressed = false;
        self.middle_click_pressed = false;
        self.stick_click_consumed = false;
        self.launcher_pressed = false;
        self.maximize_right_chord_pressed = false;
    }

    pub(super) fn update(
        &mut self,
        actions: &Actions,
        session: &xr::Session<xr::Vulkan>,
        space: &xr::Space,
        frame_state: &xr::FrameState,
        gaze_ray: Option<Ray3>,
        player: glam::Vec3,
        active_fullscreen_panel: Option<PanelId>,
        config: &AppConfig,
        input: &crate::bridge::InputSender,
        timings: &mut crate::timing::Timings,
        panels: &mut PanelImages,
    ) -> Result<()> {
        let right_hand = actions.right_hand;
        let left_hand = actions.left_hand;
        timings.measure("openxr/sync-actions", Duration::ZERO, || {
            session.sync_actions(&[xr::ActiveActionSet::new(&actions.action_set)])
        })?;
        let mut tracked_this_frame = false;
        let time_ms = (frame_state.predicted_display_time.as_nanos() / 1_000_000) as u32;
        if timings.measure("openxr/action-state", Duration::ZERO, || {
            actions.aim_action.is_active(session, right_hand)
        })? {
            let controller = timings.measure("openxr/locate-controller", Duration::ZERO, || {
                actions
                    .aim_space
                    .locate(space, frame_state.predicted_display_time)
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
                self.cursor_ray = Some(Ray3 {
                    origin: glam::Vec3::new(pose.position.x, pose.position.y, pose.position.z),
                    direction,
                });
                let _ = input.try_send(XrInput::Ray {
                    ray: self.cursor_ray.expect("ray assigned above"),
                    gaze_ray,
                    time_ms,
                });
                let resize_reach_px = config.window.grab_reach_px();
                let nearest_proximity = self.cursor_ray.and_then(|ray| {
                    panels
                        .iter()
                        .filter(|(id, _)| {
                            active_fullscreen_panel.is_none_or(|fullscreen| **id == fullscreen)
                        })
                        .filter_map(|(id, panel)| {
                            let hit = panel.geometry.trace(
                                ray,
                                resize_reach_px.max(config.window.cursor_proximity_radius_px),
                            )?;
                            if !hit.on_panel {
                                return None;
                            }
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
                self.cursor_close_panel = nearest_proximity
                    .filter(|(id, hit)| {
                        let Some(panel) = panels.get(id) else {
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
                    actions.trigger_action.state(session, right_hand)
                })?;
                let launcher = timings.measure("openxr/action-state", Duration::ZERO, || {
                    actions.launcher_action.state(session, right_hand)
                })?;
                let grip = timings.measure("openxr/action-state", Duration::ZERO, || {
                    actions.grip_action.state(session, right_hand)
                })?;
                if grip.changed_since_last_sync {
                    if grip.current_state {
                        self.grabbed_panel = self.cursor_ray.and_then(|ray| {
                            panels
                                .iter()
                                .filter(|(id, _)| {
                                    active_fullscreen_panel
                                        .is_none_or(|fullscreen| **id == fullscreen)
                                })
                                .filter_map(|(id, panel)| {
                                    panel
                                        .geometry
                                        .trace(ray, config.window.grab_reach_px())
                                        .filter(|hit| hit.on_panel)
                                        .map(|hit| (*id, hit))
                                })
                                .min_by(|(_, first), (_, second)| {
                                    first.distance_m.total_cmp(&second.distance_m)
                                })
                                .map(|(id, _hit)| {
                                    let panel = panels.get_mut(&id).expect("hit panel exists");
                                    panel.resize_pending = false;
                                    self.grab_radius =
                                        panel.geometry.pose.center.distance(player).clamp(0.6, 5.0);
                                    self.grab_initial_radius = self.grab_radius;
                                    self.grab_initial_width = panel.geometry.pose.width_m;
                                    let aim_angles = PanelPose::spherical_angles(ray.direction);
                                    let center_angles = PanelPose::spherical_angles(
                                        panel.geometry.pose.center - player,
                                    );
                                    self.grab_direction_offset = glam::Vec2::new(
                                        PanelPose::wrap_angle(center_angles.x - aim_angles.x),
                                        center_angles.y - aim_angles.y,
                                    );
                                    id
                                })
                        });
                    } else if self.grabbed_panel.take().is_some() {
                        save_dodge_targets(panels, input)?;
                    }
                }
                if trigger.changed_since_last_sync
                    && trigger.current_state
                    && self.grabbed_panel.is_none()
                    && let Some(panel_id) = hovered_panel
                    && active_fullscreen_panel.is_none_or(|fullscreen| fullscreen == panel_id)
                    && let Some(panel) = panels.get(&panel_id)
                    && let Some(ray) = self.cursor_ray
                    && let Some(hit) = panel
                        .geometry
                        .trace(ray, resize_reach_px)
                        .filter(|hit| hit.on_panel)
                {
                    self.resize_edges = hit.on_edges;
                    if self.resize_edges.into_iter().any(|edge| edge) {
                        self.resizing_panel = Some(panel_id);
                        self.resize_density = Some(panel.geometry.pixels_per_degree());
                        self.resize_size = glam::Vec2::new(
                            panel.geometry.logical_size.w as f32,
                            panel.geometry.logical_size.h as f32,
                        );
                        self.resize_previous_ray = Some(ray);
                        self.resize_requested_size = None;
                        if let Some(panel) = panels.get_mut(&panel_id) {
                            panel.resize_pending = true;
                        }
                    }
                }
                if trigger.changed_since_last_sync && !trigger.current_state {
                    finish_resize(
                        panels,
                        input,
                        &mut self.resizing_panel,
                        self.resize_density,
                        self.resize_requested_size,
                    )?;
                    self.resize_density = None;
                    self.resize_requested_size = None;
                }
                if trigger.is_active
                    && trigger.current_state
                    && let Some(panel_id) = self.resizing_panel
                    && let Some(panel) = panels.get(&panel_id)
                    && let Some(previous_ray) = self.resize_previous_ray
                    && let Some(ray) = self.cursor_ray
                    && let Some(new_size) = panel.geometry.resized_size_from_rays(
                        previous_ray,
                        ray,
                        self.resize_size,
                        self.resize_edges,
                    )
                {
                    self.resize_previous_ray = Some(ray);
                    self.resize_size = new_size;
                    let new_size = (
                        self.resize_size.x.round() as i32,
                        self.resize_size.y.round() as i32,
                    );
                    if self.resize_requested_size != Some(new_size) {
                        self.resize_requested_size = Some(new_size);
                        let _ = input.try_send(XrInput::ResizePanel {
                            panel_id,
                            width: new_size.0,
                            height: new_size.1,
                            pixels_per_degree: self.resize_density,
                        });
                    }
                }
                let delta_seconds = (frame_state.predicted_display_period.as_nanos() as f32
                    / 1_000_000_000.0)
                    .clamp(0.0, 0.1);
                if let Some(panel_id) = self.grabbed_panel {
                    let stick = timings
                        .measure("openxr/action-state", Duration::ZERO, || {
                            actions.stick_action.state(session, right_hand)
                        })?
                        .current_state;
                    self.grab_radius =
                        (self.grab_radius + stick.y * delta_seconds * 1.5).clamp(0.6, 5.0);
                    if let Some(ray) = self.cursor_ray
                        && let Some(panel) = panels.get_mut(&panel_id)
                    {
                        let mut pose = PanelPose::on_sphere_from_aim(
                            ray.direction,
                            self.grab_direction_offset,
                            self.grab_radius,
                            player,
                        );
                        pose.width_m = PanelPose::width_for_distance(
                            self.grab_initial_width,
                            self.grab_initial_radius,
                            self.grab_radius,
                        );
                        panel.geometry.pose = pose;
                        let _ = input.try_send(XrInput::MovePanel { panel_id, pose });
                    }
                } else if pointing_at_window {
                    let stick = timings
                        .measure("openxr/action-state", Duration::ZERO, || {
                            actions.stick_action.state(session, right_hand)
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
                    actions.face_click_action.state(session, left_hand)
                })?;
                let left_secondary =
                    timings.measure("openxr/action-state", Duration::ZERO, || {
                        actions.secondary_action.state(session, left_hand)
                    })?;
                let right_chord_down = grip.is_active
                    && grip.current_state
                    && self.grabbed_panel.is_some()
                    && launcher.is_active
                    && launcher.current_state;
                let right_chord_was_down = self.maximize_right_chord_pressed;
                if right_chord_down
                    && !right_chord_was_down
                    && let Some(panel_id) = self.grabbed_panel
                {
                    input.send(XrInput::ToggleMaximize { panel_id })?;
                }
                let left_chord_down = grip.is_active
                    && grip.current_state
                    && self.grabbed_panel.is_some()
                    && left_secondary.is_active
                    && left_secondary.current_state;
                if left_chord_down
                    && left_secondary.changed_since_last_sync
                    && let Some(panel_id) = self.grabbed_panel
                {
                    input.send(XrInput::ToggleMaximize { panel_id })?;
                }
                let right_face = timings.measure("openxr/action-state", Duration::ZERO, || {
                    actions.face_click_action.state(session, right_hand)
                })?;
                let stick_click = timings.measure("openxr/action-state", Duration::ZERO, || {
                    actions.stick_click_action.state(session, right_hand)
                })?;
                let stick_click_down = stick_click.is_active && stick_click.current_state;
                if !stick_click_down {
                    self.stick_click_consumed = false;
                } else if self.grabbed_panel.is_some() {
                    self.stick_click_consumed = true;
                }
                if stick_click.changed_since_last_sync
                    && stick_click_down
                    && let Some(panel_id) = self.grabbed_panel.take()
                {
                    input.send(XrInput::ClosePanel { panel_id })?;
                    save_dodge_targets_except(panels, input, Some(panel_id))?;
                }
                let left_click_down =
                    (trigger.is_active && trigger.current_state && self.resizing_panel.is_none())
                        || (left_face.is_active && left_face.current_state)
                        || (right_face.is_active && right_face.current_state);
                let middle_click_down =
                    stick_click_down && self.grabbed_panel.is_none() && !self.stick_click_consumed;
                if launcher.is_active
                    && launcher.current_state
                    && !self.launcher_pressed
                    && !right_chord_down
                    && !right_chord_was_down
                {
                    input.send(XrInput::LauncherToggle)?;
                }
                self.launcher_pressed = launcher.is_active && launcher.current_state;
                for (button, down, previous) in [
                    (0x110, left_click_down, &mut self.left_click_pressed),
                    (0x112, middle_click_down, &mut self.middle_click_pressed),
                ] {
                    if down != *previous {
                        input.send(XrInput::Ray {
                            ray: self.cursor_ray.expect("tracked ray assigned"),
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
                self.maximize_right_chord_pressed = right_chord_down;
            }
        } else {
            self.cursor_ray = None;
            if self.grabbed_panel.take().is_some() {
                save_dodge_targets_except(panels, input, self.resizing_panel)?;
            }
            finish_resize(
                panels,
                input,
                &mut self.resizing_panel,
                self.resize_density,
                self.resize_requested_size,
            )?;
            self.resize_density = None;
            self.resize_requested_size = None;
            self.cursor_close_panel = None;
        }
        if !tracked_this_frame {
            self.cursor_ray = None;
            if self.grabbed_panel.take().is_some() {
                save_dodge_targets_except(panels, input, self.resizing_panel)?;
            }
            finish_resize(
                panels,
                input,
                &mut self.resizing_panel,
                self.resize_density,
                self.resize_requested_size,
            )?;
            self.resize_density = None;
            self.resize_requested_size = None;
            if self.pointer_tracked {
                input.send(XrInput::PointerLost { time_ms })?;
                self.reset_tracking();
            }
        }
        self.pointer_tracked = tracked_this_frame;
        if !tracked_this_frame && let Some(ray) = gaze_ray {
            let _ = input.try_send(XrInput::GazeRay { ray });
        }
        Ok(())
    }
}
