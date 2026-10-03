use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
        wl_data_offer, wl_data_source, wl_keyboard, wl_pointer, wl_region, wl_registry, wl_seat,
        wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
    },
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

use crate::{Compositor, Display};

#[derive(Default)]
pub(super) struct TestClient {
    pub(super) compositor: Option<wl_compositor::WlCompositor>,
    pub(super) subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    pub(super) shm: Option<wl_shm::WlShm>,
    pub(super) shell: Option<xdg_wm_base::XdgWmBase>,
    pub(super) pointer: Option<wl_pointer::WlPointer>,
    pub(super) dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    pub(super) syncs: usize,
    pub(super) frames: usize,
    pub(super) pointer_frames: usize,
    pub(super) entered: bool,
    pub(super) keyboard_entered: bool,
    pub(super) seat: Option<wl_seat::WlSeat>,
    pub(super) keys: Vec<(u32, wayland_client::WEnum<wl_keyboard::KeyState>)>,
    pub(super) modifier_masks: Vec<u32>,
    pub(super) pointer_surface: Option<wl_surface::WlSurface>,
    pub(super) popup_configures: Vec<(i32, i32, i32, i32)>,
    pub(super) toplevel_configures: Vec<(i32, i32)>,
    pub(super) popup_done: usize,
    pub(super) repositioned: Vec<u32>,
    pub(super) motions: Vec<(f64, f64)>,
    pub(super) buttons: Vec<wayland_client::WEnum<wl_pointer::ButtonState>>,
    pub(super) axis_values: Vec<f64>,
    pub(super) input_events: Vec<&'static str>,
    pub(super) data_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    pub(super) data_device: Option<wl_data_device::WlDataDevice>,
    pub(super) selection_offer: Option<wl_data_offer::WlDataOffer>,
    pub(super) selection_mime_types: Vec<String>,
    pub(super) selection_data: Vec<u8>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for TestClient {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "wl_data_device_manager" => {
                    state.data_manager = Some(registry.bind(name, version.min(3), qh, ()))
                }
                "wl_subcompositor" => state.subcompositor = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.shell = Some(registry.bind(name, version.min(6), qh, ())),
                "zwp_linux_dmabuf_v1" => state.dmabuf = Some(registry.bind(name, 3, qh, ())),
                "wl_seat" => {
                    let seat: wl_seat::WlSeat = registry.bind(name, version.min(5), qh, ());
                    state.pointer = Some(seat.get_pointer(qh, ()));
                    seat.get_keyboard(qh, ());
                    state.seat = Some(seat);
                }
                _ => {}
            }
            if state.data_device.is_none()
                && let (Some(manager), Some(seat)) = (&state.data_manager, &state.seat)
            {
                state.data_device = Some(manager.get_data_device(seat, qh, ()));
            }
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for TestClient {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_device::Event::Selection { id } = event {
            state.selection_offer = id;
        }
    }

    wayland_client::event_created_child!(TestClient, wl_data_device::WlDataDevice, [0 => (wl_data_offer::WlDataOffer, ())]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for TestClient {
    fn event(
        state: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.selection_mime_types.push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for TestClient {
    fn event(
        state: &mut Self,
        _: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_source::Event::Send { fd, .. } = event {
            use std::io::Write;
            std::fs::File::from(fd)
                .write_all(&state.selection_data)
                .unwrap();
        }
    }
}

delegate_noop!(TestClient: ignore wl_data_device_manager::WlDataDeviceManager);

impl Dispatch<wl_callback::WlCallback, bool> for TestClient {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        frame: &bool,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if *frame {
            state.frames += 1;
        } else {
            state.syncs += 1;
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for TestClient {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                state.entered = true;
                state.pointer_surface = Some(surface);
                state.motions.push((surface_x, surface_y));
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.motions.push((surface_x, surface_y)),
            wl_pointer::Event::Button { state: button, .. } => {
                if button == wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed) {
                    state.input_events.push("button-press");
                }
                state.buttons.push(button);
            }
            wl_pointer::Event::Axis {
                axis: wayland_client::WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } => state.axis_values.push(value),
            wl_pointer::Event::Frame => state.pointer_frames += 1,
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for TestClient {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Enter { .. } = event {
            state.keyboard_entered = true;
            state.input_events.push("keyboard-enter");
        } else if let wl_keyboard::Event::Key {
            key,
            state: key_state,
            ..
        } = event
        {
            state.keys.push((key, key_state));
        } else if let wl_keyboard::Event::Modifiers { mods_depressed, .. } = event {
            state.modifier_masks.push(mods_depressed);
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for TestClient {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                state.popup_configures.push((x, y, width, height));
            }
            xdg_popup::Event::PopupDone => state.popup_done += 1,
            xdg_popup::Event::Repositioned { token } => state.repositioned.push(token),
            _ => {}
        }
    }
}

delegate_noop!(TestClient: ignore xdg_positioner::XdgPositioner);

impl Dispatch<xdg_surface::XdgSurface, ()> for TestClient {
    fn event(
        _: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
        }
    }
}

delegate_noop!(TestClient: ignore wl_compositor::WlCompositor);
delegate_noop!(TestClient: ignore wl_surface::WlSurface);
delegate_noop!(TestClient: ignore wl_region::WlRegion);
delegate_noop!(TestClient: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(TestClient: ignore wl_subsurface::WlSubsurface);
delegate_noop!(TestClient: ignore wl_shm::WlShm);
delegate_noop!(TestClient: ignore wl_shm_pool::WlShmPool);
delegate_noop!(TestClient: ignore wl_buffer::WlBuffer);
delegate_noop!(TestClient: ignore wl_seat::WlSeat);
delegate_noop!(TestClient: ignore xdg_wm_base::XdgWmBase);
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for TestClient {
    fn event(
        state: &mut Self,
        _toplevel: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            state.toplevel_configures.push((width, height));
        }
    }
}
delegate_noop!(TestClient: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
delegate_noop!(TestClient: ignore zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1);

pub(super) fn pump(
    display: &mut Display<Compositor>,
    compositor: &mut Compositor,
    queue: &mut EventQueue<TestClient>,
    client: &mut TestClient,
    connection: &Connection,
) {
    let target = client.syncs + 1;
    connection.display().sync(&queue.handle(), false);
    connection.flush().unwrap();
    display.dispatch_clients(compositor).unwrap();
    compositor.render_pending().unwrap();
    compositor.flush_clients();
    while client.syncs < target {
        queue.blocking_dispatch(client).unwrap();
    }
}
