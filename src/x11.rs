use std::process::Stdio;

use smithay::{
    backend::input::KeyState,
    delegate_xwayland_shell,
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    },
    reexports::{
        calloop::LoopHandle,
        wayland_server::{DisplayHandle, protocol::wl_surface::WlSurface},
    },
    utils::{IsAlive, Logical, Rectangle, SERIAL_COUNTER, Serial},
    wayland::{
        shell::xdg::ToplevelSurface,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge, XwmId},
    },
};

use crate::{Compositor, ToplevelPanel, bridge::PanelUpdate, panel::PanelPose};

pub struct X11Popup {
    pub window: X11Surface,
    pub surface: WlSurface,
    pub parent: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PanelSurface {
    Wayland(ToplevelSurface),
    Popup(WlSurface),
    X11 {
        window: X11Surface,
        surface: WlSurface,
    },
}

impl PanelSurface {
    pub fn wl_surface(&self) -> &WlSurface {
        match self {
            Self::Wayland(surface) => surface.wl_surface(),
            Self::Popup(surface) => surface,
            Self::X11 { surface, .. } => surface,
        }
    }

    pub fn set_activated(&self, activated: bool) -> anyhow::Result<()> {
        match self {
            Self::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                    if activated {
                        state.states.set(State::Activated);
                    } else {
                        state.states.unset(State::Activated);
                    }
                });
                surface.send_configure();
            }
            Self::X11 { window, .. } if !window.is_override_redirect() => {
                window.set_activated(activated)?;
            }
            Self::X11 { .. } | Self::Popup(_) => {}
        }
        Ok(())
    }
}

impl IsAlive for PanelSurface {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(surface) => surface.alive(),
            Self::Popup(surface) => surface.alive(),
            Self::X11 { window, .. } => window.alive(),
        }
    }
}

impl smithay::wayland::seat::WaylandFocus for PanelSurface {
    fn wl_surface(&self) -> Option<std::borrow::Cow<'_, WlSurface>> {
        Some(std::borrow::Cow::Borrowed(self.wl_surface()))
    }
}

impl From<smithay::desktop::PopupKind> for PanelSurface {
    fn from(popup: smithay::desktop::PopupKind) -> Self {
        Self::Popup(popup.wl_surface().clone())
    }
}

impl From<PanelSurface> for WlSurface {
    fn from(surface: PanelSurface) -> Self {
        surface.wl_surface().clone()
    }
}

impl KeyboardTarget<Compositor> for PanelSurface {
    fn enter(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::enter(surface.wl_surface(), seat, data, keys, serial)
            }
            Self::X11 { window, .. } => KeyboardTarget::enter(window, seat, data, keys, serial),
            Self::Popup(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<Compositor>, data: &mut Compositor, serial: Serial) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::leave(surface.wl_surface(), seat, data, serial)
            }
            Self::X11 { window, .. } => KeyboardTarget::leave(window, seat, data, serial),
            Self::Popup(surface) => KeyboardTarget::leave(surface, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::key(surface.wl_surface(), seat, data, key, state, serial, time)
            }
            Self::X11 { window, .. } => {
                KeyboardTarget::key(window, seat, data, key, state, serial, time)
            }
            Self::Popup(surface) => {
                KeyboardTarget::key(surface, seat, data, key, state, serial, time)
            }
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::modifiers(surface.wl_surface(), seat, data, modifiers, serial)
            }
            Self::X11 { window, .. } => {
                KeyboardTarget::modifiers(window, seat, data, modifiers, serial)
            }
            Self::Popup(surface) => {
                KeyboardTarget::modifiers(surface, seat, data, modifiers, serial)
            }
        }
    }
}

pub fn start(
    display: &DisplayHandle,
    handle: LoopHandle<'static, Compositor>,
) -> anyhow::Result<Option<u32>> {
    let (xwayland, client) = match XWayland::spawn(
        display,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::inherit(),
        |_| (),
    ) {
        Ok(instance) => instance,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Xwayland is not installed; X11 applications are unavailable");
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let display_number = xwayland.display_number();
    let wm_handle = handle.clone();
    handle
        .insert_source(xwayland, move |event, _, compositor| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                match X11Wm::start_wm(wm_handle.clone(), x11_socket, client.clone()) {
                    Ok(wm) => compositor.xwm = Some(wm),
                    Err(error) => {
                        compositor.fatal_error =
                            Some(anyhow::anyhow!("XWayland window manager failed: {error}"));
                        return;
                    }
                }
                let display = format!(":{display_number}");
                eprintln!("X11 display: {display}");
                if let Err(error) = compositor.notify_ready(Some(display)) {
                    compositor.fatal_error = Some(error);
                }
            }
            XWaylandEvent::Error => {
                compositor.fatal_error = Some(anyhow::anyhow!("XWayland failed to start"));
            }
        })
        .map_err(|error| anyhow::anyhow!("failed to register XWayland: {error}"))?;
    Ok(Some(display_number))
}

impl Compositor {
    fn add_x11_panel(&mut self, window: X11Surface, surface: WlSurface) {
        if window.is_override_redirect() {
            let explicit_parent = window.is_transient_for().map(|parent| {
                self.x11_popups
                    .iter()
                    .find(|popup| popup.window.window_id() == parent)
                    .map(|popup| popup.parent)
                    .unwrap_or(parent)
            });
            let parent = explicit_parent.and_then(|parent| self.panels.iter().find(|panel|
                matches!(&panel.surface, PanelSurface::X11 { window, .. } if window.window_id() == parent)))
                .or_else(|| self.panels.iter().find(|panel| Some(panel.id) == self.active_panel
                    && matches!(panel.surface, PanelSurface::X11 { .. })))
                .or_else(|| self.panels.iter().find(|panel| match &panel.surface {
                    PanelSurface::X11 { window: parent, .. } => parent.geometry().overlaps(window.geometry()),
                    _ => false,
                }));
            if let Some(parent) = parent {
                let PanelSurface::X11 {
                    window: parent_window,
                    ..
                } = &parent.surface
                else {
                    unreachable!()
                };
                let parent_id = parent_window.window_id();
                let root = parent.surface.wl_surface().clone();
                if self.x11_popups.iter().all(|popup| popup.surface != surface) {
                    self.output.enter(&surface);
                    self.x11_popups.push(X11Popup {
                        window,
                        surface,
                        parent: parent_id,
                    });
                }
                if let Some(index) = self.update_panel_from_commit(&root) {
                    self.invalidate_panel(index);
                }
                return;
            }
        }
        if self
            .panels
            .iter()
            .any(|panel| panel.surface.wl_surface() == &surface)
        {
            return;
        }
        self.output.enter(&surface);
        let pose = (0..=self.panels.len())
            .map(|slot| PanelPose::for_slot_at_distance(slot, self.default_window_distance))
            .find(|pose| self.panels.iter().all(|panel| panel.pose != *pose))
            .expect("an unused panel placement exists");
        self.panels.push(ToplevelPanel {
            surface: PanelSurface::X11 {
                window,
                surface: surface.clone(),
            },
            pose,
            geometry: None,
            id: self.next_panel_id,
            bounds: Rectangle::default(),
        });
        self.next_panel_id = self.next_panel_id.saturating_add(1);
        if let Some(index) = self.update_panel_from_commit(&surface) {
            self.invalidate_panel(index);
        }
    }

    fn remove_x11_panel(&mut self, window: &X11Surface) {
        if let Some(index) = self
            .x11_popups
            .iter()
            .position(|popup| popup.window.window_id() == window.window_id())
        {
            let popup = self.x11_popups.remove(index);
            let root = self
                .panels
                .iter()
                .find(|panel| {
                    matches!(&panel.surface,
                PanelSurface::X11 { window, .. } if window.window_id() == popup.parent)
                })
                .map(|panel| panel.surface.wl_surface().clone());
            if let Some(root) = root
                && let Some(index) = self.update_panel_from_commit(&root)
            {
                self.invalidate_panel(index);
            }
            return;
        }
        self.x11_popups
            .retain(|popup| popup.parent != window.window_id());
        self.panels.retain(|panel| {
            let remove = matches!(&panel.surface, PanelSurface::X11 { window: candidate, .. }
                if candidate.xwm_id() == window.xwm_id() && candidate.window_id() == window.window_id());
            if remove {
                self.frame_sender.publish(PanelUpdate::Removed { panel_id: panel.id });
            }
            !remove
        });
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

impl XWaylandShellHandler for Compositor {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm: XwmId, surface: WlSurface, window: X11Surface) {
        self.add_x11_panel(window, surface);
    }
}

impl XwmHandler for Compositor {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("XWayland window manager is initialized")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(error) = window
            .configure(window.geometry())
            .and_then(|()| window.set_mapped(true))
        {
            eprintln!("failed to map X11 window: {error}");
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(surface) = window.wl_surface() {
            self.add_x11_panel(window, surface);
        }
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.remove_x11_panel(&window);
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.remove_x11_panel(&window);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let mut geometry = window.geometry();
        geometry.loc.x = x.unwrap_or(geometry.loc.x);
        geometry.loc.y = y.unwrap_or(geometry.loc.y);
        geometry.size.w = width
            .map(|value| value.clamp(1, i32::MAX as u32) as i32)
            .unwrap_or(geometry.size.w);
        geometry.size.h = height
            .map(|value| value.clamp(1, i32::MAX as u32) as i32)
            .unwrap_or(geometry.size.h);
        if let Err(error) = window.configure(geometry) {
            eprintln!("failed to configure X11 window: {error}");
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        if let Some(surface) = window.wl_surface() {
            let root = self.root_surface(&surface);
            if let Some(index) = self.update_panel_from_commit(&root) {
                self.invalidate_panel(index);
            }
        }
    }
    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _edge: ResizeEdge,
    ) {
    }
    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}

    fn disconnected(&mut self, _xwm: XwmId) {
        self.fatal_error = Some(anyhow::anyhow!("XWayland disconnected"));
    }
}

delegate_xwayland_shell!(Compositor);
