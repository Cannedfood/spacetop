use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Instant,
};

use anyhow::Context;
mod bridge;
mod config;
mod gpu;
mod input;
mod panel;
mod runtime;
mod scene;
mod timing;
mod x11;
mod xr;

pub use runtime::{DisplayNames, run, run_xr_client};

#[cfg(test)]
use smithay::reexports::wayland_server::Display;

use bridge::{PanelUpdate, XrInput};
use panel::{PanelGeometry, PanelPose, Ray3};
use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, KeyState},
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat,
    delegate_shm, delegate_xdg_shell,
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, WindowSurfaceType,
        find_popup_root_surface,
        utils::{bbox_from_surface_tree, under_from_surface_tree},
    },
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, XkbConfig},
        pointer::AxisFrame,
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::LoopSignal,
        wayland_server::{
            Client, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
        },
    },
    utils::{IsAlive, Logical, Physical, Point, Raw, Rectangle, SERIAL_COUNTER, Serial, Size},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, get_parent, with_states, with_surface_tree_downward,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
            XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        xwayland_shell::XWaylandShellState,
    },
    xwayland::{X11Wm, XWaylandClientData},
};
use x11::PanelSurface;

const MOUSE_CURSOR_IDLE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum CursorSource {
    Controller,
    Mouse,
}

#[derive(Default)]
struct ClientState {
    compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

struct ToplevelPanel {
    surface: PanelSurface,
    pose: PanelPose,
    geometry: Option<PanelGeometry>,
    pose_is_explicit: bool,
    resize_anchor: Option<(PanelGeometry, [bool; 4])>,
    id: u64,
    bounds: Rectangle<i32, Logical>,
}

struct Compositor {
    display_handle: DisplayHandle,
    compositor_state: CompositorState,
    shm_state: ShmState,
    data_device_state: DataDeviceState,
    xdg_shell_state: XdgShellState,
    xwayland_shell_state: XWaylandShellState,
    xwm: Option<X11Wm>,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
    output: Output,
    panels: Vec<ToplevelPanel>,
    gpu_renderer: Option<gpu::GpuRenderer>,
    dmabuf_state: DmabufState,
    dmabuf_global: Option<DmabufGlobal>,
    next_panel_id: u64,
    frame_sender: bridge::PanelSender,
    panel_limits: panel::PanelLimits,
    default_window_distance: f32,
    window_pixels_per_degree: f32,
    active_panel: Option<u64>,
    fatal_error: Option<anyhow::Error>,
    started_at: Instant,
    popups: PopupManager,
    input_serials: VecDeque<(Serial, WlSurface)>,
    x11_popups: Vec<x11::X11Popup>,
    xr_buttons: BTreeSet<u32>,
    key_counts: BTreeMap<u32, usize>,
    button_counts: BTreeMap<u32, usize>,
    gaze_ray: Option<Ray3>,
    controller_ray: Option<Ray3>,
    mouse_base_ray: Option<Ray3>,
    mouse_angles: glam::Vec2,
    cursor_source: CursorSource,
    mouse_last_moved: Option<Instant>,
    mouse_cursor_visible: bool,
    ready_callback: Option<runtime::ReadyCallback>,
    dirty_panels: BTreeSet<u64>,
    frame_requested: bool,
    timings: timing::Timings,
}

impl Compositor {
    fn notify_ready(&mut self, x11_display: Option<String>) -> anyhow::Result<()> {
        if let Some(callback) = self.ready_callback.take() {
            callback(x11_display)?;
        }
        Ok(())
    }

    fn finish_dispatch(&mut self, signal: &LoopSignal) {
        self.popups.cleanup();
        if self.frame_requested {
            self.frame_requested = false;
            if let Err(error) = self.render_pending() {
                self.fatal_error = Some(error);
            }
        }
        self.flush_clients();
        if self.fatal_error.is_some() {
            signal.stop();
        }
    }

    fn handle_xr_input(&mut self, command: XrInput) {
        let result = match command {
            XrInput::FrameTick => {
                self.frame_requested = true;
                Ok(())
            }
            XrInput::Ray {
                ray,
                gaze_ray,
                time_ms,
            } => {
                self.dispatch_controller_ray(ray, gaze_ray, time_ms);
                Ok(())
            }
            XrInput::GazeRay { ray } => {
                self.gaze_ray = Some(ray);
                Ok(())
            }
            XrInput::Button {
                button,
                pressed,
                time_ms,
            } => {
                let changed = if pressed {
                    self.xr_buttons.insert(button)
                } else {
                    self.xr_buttons.remove(&button)
                };
                if changed {
                    self.dispatch_pointer_button(button, pressed, time_ms);
                }
                Ok(())
            }
            XrInput::PointerLost { time_ms } => {
                for button in std::mem::take(&mut self.xr_buttons) {
                    self.dispatch_pointer_button(button, false, time_ms);
                }
                if self.cursor_source == CursorSource::Controller
                    && let Some(pointer) = self.seat.get_pointer()
                {
                    pointer.motion(
                        self,
                        None,
                        &smithay::input::pointer::MotionEvent {
                            location: pointer.current_location(),
                            serial: SERIAL_COUNTER.next_serial(),
                            time: time_ms,
                        },
                    );
                    pointer.frame(self);
                }
                Ok(())
            }
            XrInput::Scroll { value, time_ms } => {
                self.dispatch_scroll(value, time_ms);
                Ok(())
            }
            XrInput::ResizePanel {
                panel_id,
                width,
                height,
                anchor,
            } => {
                if let Some(panel) = self.panels.iter_mut().find(|panel| panel.id == panel_id) {
                    if let Some(anchor) = anchor {
                        panel.resize_anchor = Some(anchor);
                        panel.pose_is_explicit = true;
                    }
                    match &panel.surface {
                        x11::PanelSurface::X11 { window, .. } => {
                            let mut geometry = window.geometry();
                            geometry.size.w = width.max(1);
                            geometry.size.h = height.max(1);
                            if let Err(error) = window.configure(geometry) {
                                eprintln!("failed to resize X11 window: {error}");
                            }
                        }
                        x11::PanelSurface::Wayland(surface) => {
                            let window_bounds = with_states(surface.wl_surface(), |states| {
                                states
                                    .cached_state
                                    .get::<SurfaceCachedState>()
                                    .current()
                                    .geometry
                            })
                            .map(|geometry| {
                                geometry.intersection(panel.bounds).unwrap_or(panel.bounds)
                            })
                            .unwrap_or_else(|| {
                                bbox_from_surface_tree(surface.wl_surface(), (0, 0))
                            });
                            let padding = panel.bounds.size - window_bounds.size;
                            surface.with_pending_state(|state| {
                                state.size = Some(
                                    ((width - padding.w).max(1), (height - padding.h).max(1))
                                        .into(),
                                )
                            });
                            surface.send_configure();
                        }
                        x11::PanelSurface::Popup(_) => {}
                    }
                }
                Ok(())
            }
            XrInput::MovePanel { panel_id, pose } => {
                if let Some(panel) = self.panels.iter_mut().find(|panel| panel.id == panel_id)
                    && let Some(geometry) = panel.geometry.as_mut()
                {
                    geometry.pose = pose;
                    panel.pose_is_explicit = true;
                    panel.resize_anchor = None;
                    if let Some(root_size) =
                        smithay::backend::renderer::utils::with_renderer_surface_state(
                            panel.surface.wl_surface(),
                            |state| state.surface_size(),
                        )
                        .flatten()
                    {
                        panel.pose = geometry.root_pose(root_size, panel.bounds);
                    }
                }
                Ok(())
            }
            XrInput::GpuDevice {
                render_node,
                limits,
            } => {
                self.panel_limits = limits;
                self.configure_gpu(&render_node)
            }
            XrInput::ConfigReloaded {
                default_window_distance,
                window_pixels_per_degree,
            } => {
                if self.default_window_distance != default_window_distance
                    || self.window_pixels_per_degree != window_pixels_per_degree
                {
                    for panel in &mut self.panels {
                        panel.pose_is_explicit = false;
                    }
                }
                self.default_window_distance = default_window_distance;
                self.window_pixels_per_degree = window_pixels_per_degree;
                self.refresh_panels();
                Ok(())
            }
            XrInput::FatalError { message } => Err(anyhow::anyhow!(message)),
        };
        if let Err(error) = result {
            self.fatal_error = Some(error);
        }
    }

    fn flush_clients(&mut self) {
        if let Err(error) = self.display_handle.flush_clients() {
            eprintln!("failed to flush Wayland events: {error}");
        }
    }

    #[cfg(test)]
    fn new(display_handle: DisplayHandle, frame_sender: bridge::PanelSender) -> Self {
        let defaults = config::AppConfig::default();
        Self::with_window_settings(
            display_handle,
            frame_sender,
            defaults.window.default_distance_m,
            defaults.window.pixels_per_degree,
        )
    }

    fn with_window_settings(
        display_handle: DisplayHandle,
        frame_sender: bridge::PanelSender,
        default_window_distance: f32,
        window_pixels_per_degree: f32,
    ) -> Self {
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&display_handle);
        let output_manager_state = OutputManagerState::new();

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "spacetop-seat");
        seat.add_pointer();
        seat.add_keyboard(XkbConfig::default(), 200, 25)
            .expect("initialize XKB keyboard");

        let output = Output::new(
            "SPACETOP-0".into(),
            PhysicalProperties {
                size: Size::<i32, Raw>::from((0, 0)),
                subpixel: Subpixel::Unknown,
                make: "spacetop".into(),
                model: "virtual".into(),
            },
        );
        let mode = Mode {
            size: Size::<i32, Physical>::from((1280, 720)),
            refresh: 60_000,
        };
        output.set_preferred(mode);
        output.change_current_state(Some(mode), None, None, None);
        output.create_global::<Self>(&display_handle);

        Self {
            display_handle,
            compositor_state,
            shm_state,
            data_device_state,
            xdg_shell_state,
            xwayland_shell_state,
            xwm: None,
            _output_manager_state: output_manager_state,
            seat_state,
            seat,
            output,
            panels: Vec::new(),
            gpu_renderer: None,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            next_panel_id: 1,
            frame_sender,
            panel_limits: panel::PanelLimits::default(),
            default_window_distance,
            window_pixels_per_degree,
            active_panel: None,
            fatal_error: None,
            started_at: Instant::now(),
            popups: PopupManager::default(),
            input_serials: VecDeque::new(),
            x11_popups: Vec::new(),
            xr_buttons: BTreeSet::new(),
            key_counts: BTreeMap::new(),
            button_counts: BTreeMap::new(),
            gaze_ray: None,
            controller_ray: None,
            mouse_base_ray: None,
            mouse_angles: glam::Vec2::ZERO,
            cursor_source: CursorSource::Controller,
            mouse_last_moved: None,
            mouse_cursor_visible: false,
            ready_callback: None,
            dirty_panels: BTreeSet::new(),
            frame_requested: false,
            timings: timing::Timings::new(),
        }
    }

    fn root_surface(&self, surface: &WlSurface) -> WlSurface {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if let Some(popup) = self.popups.find_popup(&root)
            && let Ok(parent) = find_popup_root_surface(&popup)
        {
            root = parent;
        }
        if let Some(popup) = self.x11_popups.iter().find(|popup| popup.surface == root)
            && let Some(parent) = self.panels.iter().find(|panel| {
                matches!(&panel.surface,
                PanelSurface::X11 { window, .. } if window.window_id() == popup.parent)
            })
        {
            root = parent.surface.wl_surface().clone();
        }
        root
    }

    fn surface_geometry(surface: &WlSurface) -> Rectangle<i32, Logical> {
        let bounds = bbox_from_surface_tree(surface, (0, 0));
        with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
                .and_then(|geometry| geometry.intersection(bounds))
        })
        .unwrap_or(bounds)
    }

    fn panel_surfaces(&self, index: usize) -> Vec<(WlSurface, Point<i32, Logical>)> {
        let root = self.panels[index].surface.wl_surface();
        let origin = Self::surface_geometry(root).loc;
        let mut surfaces: Vec<_> = PopupManager::popups_for_surface(root)
            .map(|(popup, location)| {
                let offset = origin + location - popup.geometry().loc;
                (popup.wl_surface().clone(), offset)
            })
            .collect();
        if let PanelSurface::X11 { window, .. } = &self.panels[index].surface {
            surfaces.extend(
                self.x11_popups
                    .iter()
                    .rev()
                    .filter(|popup| popup.parent == window.window_id() && popup.window.is_mapped())
                    .map(|popup| {
                        (
                            popup.surface.clone(),
                            popup.window.geometry().loc - window.geometry().loc,
                        )
                    }),
            );
        }
        surfaces.push((root.clone(), (0, 0).into()));
        surfaces
    }

    fn remember_input_serial(&mut self, serial: Serial, surface: WlSurface) {
        self.input_serials.push_back((serial, surface));
        if self.input_serials.len() > 32 {
            self.input_serials.pop_front();
        }
    }

    fn update_panel_from_commit(&mut self, surface: &WlSurface) -> Option<usize> {
        let index = self
            .panels
            .iter()
            .position(|panel| panel.surface.wl_surface() == surface)?;
        let logical_size =
            smithay::backend::renderer::utils::with_renderer_surface_state(surface, |state| {
                state.surface_size()
            })
            .flatten();

        let was_mapped = self.panels[index].geometry.is_some();
        if logical_size.is_none() && was_mapped {
            self.frame_sender.publish(PanelUpdate::Removed {
                panel_id: self.panels[index].id,
            });
        }
        self.panels[index].geometry = logical_size.map(|logical_size| {
            let bounds = self.panel_surfaces(index).into_iter().fold(
                Rectangle::from_size(logical_size),
                |bounds, (surface, offset)| {
                    let child = bbox_from_surface_tree(&surface, offset);
                    if child.size.w > 0 && child.size.h > 0 {
                        bounds.merge(child)
                    } else {
                        bounds
                    }
                },
            );
            self.panels[index].bounds = bounds;
            let mut geometry =
                PanelGeometry::from_bounds(self.panels[index].pose, logical_size, bounds);
            if let Some((anchor, edges)) = self.panels[index].resize_anchor {
                geometry.pose = anchor.resized_pose_from_edges(
                    bounds.size,
                    edges,
                    self.window_pixels_per_degree,
                );
                self.panels[index].pose = geometry.root_pose(logical_size, bounds);
            }
            geometry
        });
        if self.active_panel == Some(self.panels[index].id) && logical_size.is_none() && was_mapped
        {
            let next = self
                .panels
                .iter()
                .find(|panel| panel.geometry.is_some())
                .map(|panel| panel.id);
            self.set_panel_active(next, SERIAL_COUNTER.next_serial());
        } else if self.active_panel.is_none() && logical_size.is_some() {
            self.set_panel_active(Some(self.panels[index].id), SERIAL_COUNTER.next_serial());
        }
        Some(index)
    }

    fn configure_gpu(&mut self, render_node: &std::path::Path) -> anyhow::Result<()> {
        let renderer = gpu::GpuRenderer::new(render_node)
            .context("mandatory GPU compositor initialization failed")?;
        let feedback = renderer.feedback(render_node)?;
        self.gpu_renderer = Some(renderer);
        if self.dmabuf_global.is_none() {
            self.dmabuf_global = Some(
                self.dmabuf_state
                    .create_global_with_default_feedback::<Self>(&self.display_handle, &feedback),
            );
        }
        eprintln!("GPU panel compositing enabled on {}", render_node.display());
        self.refresh_panels();
        Ok(())
    }

    fn refresh_panels(&mut self) {
        for index in 0..self.panels.len() {
            let surface = self.panels[index].surface.wl_surface().clone();
            self.update_panel_from_commit(&surface);
            self.invalidate_panel(index);
        }
    }

    fn invalidate_panel(&mut self, index: usize) {
        self.dirty_panels.insert(self.panels[index].id);
    }

    fn render_pending(&mut self) -> anyhow::Result<()> {
        if self.gpu_renderer.is_none() || self.dirty_panels.is_empty() {
            return Ok(());
        }
        let started = Instant::now();
        for panel_id in std::mem::take(&mut self.dirty_panels) {
            if let Some(index) = self.panels.iter().position(|panel| panel.id == panel_id) {
                let surface = self.panels[index].surface.wl_surface().clone();
                self.update_panel_from_commit(&surface);
                self.capture_panel(index)?;
            }
        }
        self.timings.record(
            "mixed/compositor-batch",
            started.elapsed(),
            std::time::Duration::ZERO,
        );
        Ok(())
    }

    fn complete_frame_callbacks(&self, surface: &WlSurface) {
        let time_ms = self.started_at.elapsed().as_millis() as u32;
        with_surface_tree_downward(
            surface,
            (),
            |_, _, &()| TraversalAction::DoChildren(()),
            |_, states, &()| {
                for callback in states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .frame_callbacks
                    .drain(..)
                {
                    callback.done(time_ms);
                }
            },
            |_, _, &()| true,
        );
    }

    fn capture_panel(&mut self, index: usize) -> anyhow::Result<()> {
        let surface = self.panels[index].surface.wl_surface().clone();
        let panel_id = self.panels[index].id;
        let Some((root_size, buffer_scale)) =
            smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |state| {
                state
                    .surface_size()
                    .map(|size| (size, state.buffer_scale()))
            })
            .flatten()
        else {
            return Ok(());
        };

        let Some(mut geometry) = self.panels[index].geometry else {
            return Ok(());
        };
        let bounds = self.panels[index].bounds;
        let surfaces: Vec<_> = self
            .panel_surfaces(index)
            .into_iter()
            .map(|(surface, offset)| (surface, offset - bounds.loc))
            .collect();
        let Some(renderer) = self.gpu_renderer.as_mut() else {
            return Ok(());
        };
        anyhow::ensure!(
            self.panels
                .iter()
                .filter(|panel| panel.geometry.is_some())
                .count()
                <= self.panel_limits.max_layers as usize,
            "OpenXR supports at most {} mapped window layers",
            self.panel_limits.max_layers
        );
        let (size, scale) = self
            .panel_limits
            .capture_size(geometry.logical_size, buffer_scale);
        let dmabuf = self
            .timings
            .measure("app/compositor-capture", std::time::Duration::ZERO, || {
                renderer.capture(&surfaces, size, scale)
            })
            .context("mandatory GPU panel capture failed")?;
        if !self.panels[index].pose_is_explicit {
            geometry.pose.width_m = PanelPose::width_for_pixel_density(
                size.w as f32,
                geometry.pose.center.length(),
                self.window_pixels_per_degree,
            );
        }
        self.panels[index].pose = geometry.root_pose(root_size, bounds);
        self.panels[index].geometry = Some(geometry);
        let Some(geometry) = self.panels[index].geometry else {
            return Ok(());
        };
        self.frame_sender.publish(PanelUpdate::GpuFrame {
            panel_id,
            dmabuf,
            geometry,
        });
        for (surface, _) in surfaces {
            self.complete_frame_callbacks(&surface);
        }
        Ok(())
    }

    fn set_panel_active(&mut self, panel_id: Option<u64>, serial: smithay::utils::Serial) {
        for panel in &self.panels {
            let should_activate = Some(panel.id) == panel_id;
            let currently_active = self.active_panel == Some(panel.id);
            if should_activate != currently_active
                && let Err(error) = panel.surface.set_activated(should_activate)
            {
                eprintln!("failed to update window activation: {error:#}");
            }
        }
        self.active_panel = panel_id;
        if let (Some(panel_id), Some(keyboard)) = (panel_id, self.seat.get_keyboard()) {
            if let Some(panel) = self.panels.iter().find(|panel| panel.id == panel_id) {
                keyboard.set_focus(self, Some(panel.surface.clone()), serial);
            }
        } else if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, None, serial);
            if keyboard.current_focus().is_none() {
                set_data_device_focus(&self.display_handle, &self.seat, None);
            }
        }
    }

    fn dispatch_ray(&mut self, ray: Ray3, time_ms: u32) -> bool {
        let hit = self
            .panels
            .iter()
            .filter(|panel| panel.surface.alive())
            .filter_map(|panel| {
                let geometry = panel.geometry?;
                let hit = geometry.intersect(ray)?;
                let point = Point::from((hit.surface_px.x as f64, hit.surface_px.y as f64))
                    + panel.bounds.loc.to_f64();
                let index = self
                    .panels
                    .iter()
                    .position(|candidate| candidate.id == panel.id)?;
                let (surface, offset) =
                    self.panel_surfaces(index)
                        .into_iter()
                        .find_map(|(surface, offset)| {
                            under_from_surface_tree(&surface, point, offset, WindowSurfaceType::ALL)
                        })?;
                Some((surface, offset, hit, point))
            })
            .min_by(|a, b| a.2.distance_m.total_cmp(&b.2.distance_m));
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        let focus = hit
            .as_ref()
            .map(|(surface, offset, _, _)| (surface.clone(), offset.to_f64()));
        let location = hit
            .as_ref()
            .map(|(_, _, _, point)| *point)
            .unwrap_or_else(|| pointer.current_location());
        pointer.motion(
            self,
            focus,
            &smithay::input::pointer::MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: time_ms,
            },
        );
        pointer.frame(self);
        hit.is_some()
    }

    fn dispatch_controller_ray(&mut self, ray: Ray3, gaze_ray: Option<Ray3>, time_ms: u32) {
        if let Some(gaze_ray) = gaze_ray {
            self.gaze_ray = Some(gaze_ray);
        }
        let moved = self
            .controller_ray
            .is_some_and(|previous| ray_moved(previous, ray));
        self.controller_ray = Some(ray);
        if self.cursor_source == CursorSource::Mouse && moved {
            self.cursor_source = CursorSource::Controller;
            self.mouse_cursor_visible = false;
            self.frame_sender.publish_cursor(bridge::CursorState {
                mouse_controlled: false,
                pose: None,
            });
        }
        if self.cursor_source == CursorSource::Controller {
            self.dispatch_ray(ray, time_ms);
        }
    }

    fn dispatch_mouse_motion(&mut self, dx: f32, dy: f32, time_ms: u32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        if self.cursor_source != CursorSource::Mouse {
            let Some(base_ray) = self.gaze_ray.or(self.controller_ray) else {
                return;
            };
            self.cursor_source = CursorSource::Mouse;
            self.mouse_base_ray = Some(base_ray);
            self.mouse_angles = glam::Vec2::ZERO;
        }
        let Some(base_ray) = self.mouse_base_ray else {
            return;
        };
        let radians_per_pixel =
            std::f32::consts::PI / (180.0 * self.window_pixels_per_degree.max(1.0));
        self.mouse_angles += glam::Vec2::new(dx, -dy) * radians_per_pixel;
        self.mouse_angles.x = self.mouse_angles.x.rem_euclid(2.0 * std::f32::consts::PI);
        self.mouse_angles.y = self.mouse_angles.y.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
        let direction = base_ray.direction.normalize_or_zero();
        let mut right = direction.cross(glam::Vec3::Y);
        if right.length_squared() < 1.0e-6 {
            right = direction.cross(glam::Vec3::Z);
        }
        right = right.normalize_or_zero();
        let up = right.cross(direction).normalize_or_zero();
        let (sin_yaw, cos_yaw) = self.mouse_angles.x.sin_cos();
        let (sin_pitch, cos_pitch) = self.mouse_angles.y.sin_cos();
        let mouse_ray = Ray3 {
            origin: base_ray.origin,
            direction: direction * (cos_yaw * cos_pitch)
                + right * (sin_yaw * cos_pitch)
                + up * sin_pitch,
        };
        self.mouse_last_moved = Some(Instant::now());
        self.mouse_cursor_visible = true;
        self.dispatch_ray(mouse_ray, time_ms);
        let pose = self.mouse_cursor_pose(mouse_ray);
        self.frame_sender.publish_cursor(bridge::CursorState {
            mouse_controlled: true,
            pose,
        });
    }

    fn mouse_cursor_pose(&self, ray: Ray3) -> Option<PanelPose> {
        let nearest = self
            .panels
            .iter()
            .filter(|panel| panel.surface.alive())
            .filter_map(|panel| {
                let geometry = panel.geometry?;
                geometry
                    .intersect(ray)
                    .map(|hit| (geometry.pose, hit.distance_m))
            })
            .min_by(|(_, first), (_, second)| first.total_cmp(second));
        if let Some((pose, distance)) = nearest {
            return Some(PanelPose {
                center: ray.origin + ray.direction * distance,
                width_m: 0.021,
                ..pose
            });
        }
        Some(PanelPose {
            width_m: 0.021,
            ..PanelPose::facing_player(
                ray.origin + ray.direction * self.default_window_distance,
                ray.origin,
            )
        })
    }

    pub(crate) fn hide_idle_mouse_cursor(&mut self) {
        if self.cursor_source == CursorSource::Mouse
            && self.mouse_cursor_visible
            && self
                .mouse_last_moved
                .is_some_and(|moved| moved.elapsed() >= MOUSE_CURSOR_IDLE)
        {
            self.mouse_cursor_visible = false;
            self.frame_sender.publish_cursor(bridge::CursorState {
                mouse_controlled: true,
                pose: None,
            });
        }
    }

    #[cfg(test)]
    fn dispatch_button(&mut self, pressed: bool, time_ms: u32) {
        self.dispatch_pointer_button(0x110, pressed, time_ms);
    }

    fn dispatch_key(&mut self, code: u32, pressed: bool, time_ms: u32) {
        if !input::transition(&mut self.key_counts, code, pressed) {
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        if pressed && let Some(focus) = keyboard.current_focus() {
            self.remember_input_serial(serial, focus.wl_surface().clone());
        }
        keyboard.input::<(), _>(
            self,
            (code + 8).into(),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            serial,
            time_ms,
            |_, _, _| FilterResult::Forward,
        );
    }

    fn dispatch_wheel(&mut self, horizontal: bool, steps: i32, time_ms: u32) {
        if let Some(pointer) = self.seat.get_pointer() {
            let axis = if horizontal {
                Axis::Horizontal
            } else {
                Axis::Vertical
            };
            pointer.axis(
                self,
                AxisFrame::new(time_ms)
                    .source(AxisSource::Wheel)
                    .value(axis, -steps as f64 * 15.0)
                    .v120(axis, -steps * 120),
            );
            pointer.frame(self);
        }
    }

    fn dispatch_pointer_button(&mut self, button: u32, pressed: bool, time_ms: u32) {
        if !input::transition(&mut self.button_counts, button, pressed) {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pressed && !pointer.is_grabbed() {
            self.activate_pointer_panel();
        }
        let serial = SERIAL_COUNTER.next_serial();
        let popup_roots: Vec<_> = self
            .panels
            .iter()
            .filter(|panel| {
                PopupManager::popups_for_surface(panel.surface.wl_surface())
                    .next()
                    .is_some()
            })
            .map(|panel| panel.surface.wl_surface().clone())
            .collect();
        if pressed && let Some(surface) = pointer.current_focus() {
            self.remember_input_serial(serial, surface);
        }
        pointer.button(
            self,
            &smithay::input::pointer::ButtonEvent {
                serial,
                time: time_ms,
                button,
                state: if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
            },
        );
        pointer.frame(self);
        if pressed {
            self.activate_pointer_panel();
            for root in popup_roots {
                if let Some(index) = self.update_panel_from_commit(&root) {
                    self.invalidate_panel(index);
                }
            }
        }
    }

    fn activate_pointer_panel(&mut self) {
        let panel_id = self
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.current_focus())
            .and_then(|surface| {
                let surface = self.root_surface(&surface);
                self.panels
                    .iter()
                    .find(|panel| panel.surface.wl_surface() == &surface)
                    .map(|panel| panel.id)
            });
        if panel_id != self.active_panel {
            self.set_panel_active(panel_id, SERIAL_COUNTER.next_serial());
        }
    }

    fn dispatch_scroll(&mut self, value: f64, time_ms: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pointer.current_focus().is_none() {
            return;
        }
        pointer.axis(
            self,
            AxisFrame::new(time_ms)
                .source(AxisSource::Continuous)
                .value(Axis::Vertical, value),
        );
        pointer.frame(self);
    }
}

impl CompositorHandler for Compositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(data) = client.get_data::<XWaylandClientData>() {
            return &data.compositor_state;
        }
        &client
            .get_data::<ClientState>()
            .expect("client state is installed on connection")
            .compositor
    }
    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);
        let root = self.root_surface(surface);
        if let Some(index) = self.update_panel_from_commit(&root) {
            self.invalidate_panel(index);
        }
    }
}

impl BufferHandler for Compositor {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}
impl DmabufHandler for Compositor {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: ImportNotifier,
    ) {
        if self
            .gpu_renderer
            .as_mut()
            .is_some_and(|renderer| renderer.import_dmabuf(&dmabuf))
        {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}
impl ShmHandler for Compositor {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.output.enter(surface.wl_surface());
        let panel_id = self.next_panel_id;
        let pose = (0..=self.panels.len())
            .map(|slot| PanelPose::for_slot_at_distance(slot, self.default_window_distance))
            .find(|pose| self.panels.iter().all(|panel| panel.pose != *pose))
            .expect("an unused panel placement exists");
        self.panels.push(ToplevelPanel {
            surface: PanelSurface::Wayland(surface.clone()),
            pose,
            geometry: None,
            pose_is_explicit: false,
            resize_anchor: None,
            id: panel_id,
            bounds: Rectangle::default(),
        });
        self.next_panel_id = self.next_panel_id.saturating_add(1);
        let activate = self.active_panel.is_none();
        if activate {
            self.set_panel_active(Some(panel_id), SERIAL_COUNTER.next_serial());
        } else {
            let _ = surface.send_configure();
        }
    }
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.output.enter(surface.wl_surface());
        let _ = self.popups.track_popup(surface.clone().into());
        let _ = surface.send_configure();
    }
    fn grab(
        &mut self,
        surface: PopupSurface,
        seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        serial: Serial,
    ) {
        let popup = PopupKind::from(surface);
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let valid = Seat::<Self>::from_resource(&seat).as_ref() == Some(&self.seat)
            && self.input_serials.iter().any(|(saved, origin)| {
                *saved == serial && origin.id().same_client_as(&popup.wl_surface().id())
            });
        if !valid {
            let _ = PopupManager::dismiss_popup(&root, &popup);
            return;
        }
        let Some(focus) = self
            .panels
            .iter()
            .find(|panel| panel.surface.wl_surface() == &root)
            .map(|panel| panel.surface.clone())
        else {
            return;
        };
        let seat = self.seat.clone();
        if let Ok(grab) = self.popups.grab_popup(focus, popup, &seat, serial) {
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.set_focus(self, grab.current_grab(), serial);
                keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            }
            if let Some(pointer) = seat.get_pointer() {
                pointer.set_grab(
                    self,
                    PopupPointerGrab::new(&grab),
                    serial,
                    smithay::input::pointer::Focus::Keep,
                );
            }
        }
    }
    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
        let _ = surface.send_configure();
    }
    fn popup_destroyed(&mut self, surface: PopupSurface) {
        let popup = PopupKind::from(surface);
        if let Ok(root) = find_popup_root_surface(&popup) {
            let _ = PopupManager::dismiss_popup(&root, &popup);
            self.popups.cleanup();
            if let Some(index) = self.update_panel_from_commit(&root) {
                self.invalidate_panel(index);
            }
        }
    }
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let removed: Vec<u64> = self
            .panels
            .iter()
            .filter(|panel| panel.surface.wl_surface() == surface.wl_surface())
            .map(|panel| panel.id)
            .collect();
        self.panels
            .retain(|panel| panel.surface.wl_surface() != surface.wl_surface());
        for panel_id in removed {
            self.frame_sender.publish(PanelUpdate::Removed { panel_id });
        }
        if self
            .active_panel
            .is_some_and(|active| self.panels.iter().all(|panel| panel.id != active))
        {
            let next = self
                .panels
                .iter()
                .find(|panel| panel.geometry.is_some())
                .map(|panel| panel.id);
            self.set_panel_active(next, SERIAL_COUNTER.next_serial());
        }
    }
}

impl SeatHandler for Compositor {
    type KeyboardFocus = PanelSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&PanelSurface>) {
        set_data_device_focus(
            &self.display_handle,
            seat,
            focused.and_then(|focus| focus.wl_surface().client()),
        );
    }
}
impl SelectionHandler for Compositor {
    type SelectionUserData = ();
}
impl ClientDndGrabHandler for Compositor {}
impl ServerDndGrabHandler for Compositor {}
impl DataDeviceHandler for Compositor {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl OutputHandler for Compositor {}

fn ray_moved(previous: Ray3, current: Ray3) -> bool {
    let previous_direction = previous.direction.normalize_or_zero();
    let current_direction = current.direction.normalize_or_zero();
    previous.origin.distance_squared(current.origin) > 0.01 * 0.01
        || previous_direction.dot(current_direction) < 0.99995
}

delegate_compositor!(Compositor);
delegate_dmabuf!(Compositor);
delegate_shm!(Compositor);
delegate_data_device!(Compositor);
delegate_xdg_shell!(Compositor);
delegate_seat!(Compositor);
delegate_output!(Compositor);

#[cfg(test)]
#[path = "../tests/compositor/mod.rs"]
mod compositor_tests;
