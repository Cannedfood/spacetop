use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Instant,
};

use anyhow::Context;
mod bridge;
mod compositor_core;
mod config;
mod gpu;
mod input;
mod panel;
mod runtime;
mod scene;
mod timing;
mod x11;
mod xr;

pub use runtime::{DisplayNames, run, run_with_config, run_xr_client};

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
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
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
    is_fullscreen: bool,
    is_maximized: bool,
    maximize_restore_size: Option<(i32, i32)>,
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
    default_vertical_angle_degrees: f32,
    window_pixels_per_degree: f32,
    window_display_scale: f32,
    maximized_max_width_degrees: f32,
    maximized_max_height_degrees: f32,
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
            .map(|slot| {
                PanelPose::for_slot_at_distance(
                    slot,
                    self.default_window_distance,
                    self.default_vertical_angle_degrees,
                )
            })
            .find(|pose| self.panels.iter().all(|panel| panel.pose != *pose))
            .expect("an unused panel placement exists");
        self.panels.push(ToplevelPanel {
            surface: PanelSurface::Wayland(surface.clone()),
            pose,
            geometry: None,
            is_fullscreen: surface.current_state().states.contains(
                smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
            ),
            is_maximized: surface.current_state().states.contains(
                smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
            ),
            maximize_restore_size: None,
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
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        if let Some(index) = self
            .panels
            .iter()
            .position(|panel| panel.surface.wl_surface() == surface.wl_surface())
            && self.panels[index].is_maximized
            && let Err(error) = self.request_panel_maximized(index, false)
        {
            eprintln!("failed to restore maximized Wayland window before fullscreen: {error:#}");
            return;
        }
        surface.with_pending_state(|state| {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
            state.states.set(State::Fullscreen);
            state.states.unset(State::Maximized);
            state.fullscreen_output = output;
        });
        let _ = surface.send_configure();
    }
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
            state.states.unset(State::Fullscreen);
            state.fullscreen_output = None;
        });
        let _ = surface.send_configure();
    }
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(index) = self
            .panels
            .iter()
            .position(|panel| panel.surface.wl_surface() == surface.wl_surface())
        {
            if let Err(error) = self.request_panel_maximized(index, true) {
                eprintln!("failed to maximize Wayland window: {error:#}");
            }
        } else {
            surface.with_pending_state(|state| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                state.states.set(State::Maximized);
                state.states.unset(State::Fullscreen);
                state.fullscreen_output = None;
            });
            let _ = surface.send_configure();
        }
    }
    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(index) = self
            .panels
            .iter()
            .position(|panel| panel.surface.wl_surface() == surface.wl_surface())
        {
            if let Err(error) = self.request_panel_maximized(index, false) {
                eprintln!("failed to unmaximize Wayland window: {error:#}");
            }
        } else {
            surface.with_pending_state(|state| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                state.states.unset(State::Maximized);
            });
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
