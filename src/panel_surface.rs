use smithay::{
    backend::input::KeyState,
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Serial},
    wayland::shell::xdg::ToplevelSurface,
    xwayland::X11Surface,
};

use crate::Compositor;

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

    pub fn set_maximized(&self, maximized: bool, size: Option<(i32, i32)>) -> anyhow::Result<()> {
        match self {
            Self::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                    if maximized {
                        state.states.set(State::Maximized);
                    } else {
                        state.states.unset(State::Maximized);
                    }
                    if let Some((width, height)) = size {
                        state.size = Some((width.max(1), height.max(1)).into());
                    }
                });
                surface.send_configure();
            }
            Self::X11 { window, .. } if !window.is_override_redirect() => {
                window.set_maximized(maximized)?;
                if let Some((width, height)) = size {
                    let mut geometry = window.geometry();
                    geometry.size.w = width.max(1);
                    geometry.size.h = height.max(1);
                    window.configure(geometry)?;
                }
            }
            Self::X11 { .. } | Self::Popup(_) => {}
        }
        Ok(())
    }

    pub fn set_fullscreen(&self, fullscreen: bool, size: Option<(i32, i32)>) -> anyhow::Result<()> {
        match self {
            Self::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                    if fullscreen {
                        state.states.set(State::Fullscreen);
                    } else {
                        state.states.unset(State::Fullscreen);
                        state.fullscreen_output = None;
                    }
                    if let Some((width, height)) = size {
                        state.size = Some((width.max(1), height.max(1)).into());
                    }
                });
                surface.send_configure();
            }
            Self::X11 { window, .. } if !window.is_override_redirect() => {
                window.set_fullscreen(fullscreen)?;
                if let Some((width, height)) = size {
                    let mut geometry = window.geometry();
                    geometry.size = (width.max(1), height.max(1)).into();
                    window.configure(geometry)?;
                }
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
