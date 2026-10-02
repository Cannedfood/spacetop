use std::{io::Write, os::unix::net::UnixStream, sync::Arc};

use glam::Vec3;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat,
        wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use super::{ClientState, Compositor, Display, Ray3, bridge};

#[derive(Default)]
struct TestClient {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    shell: Option<xdg_wm_base::XdgWmBase>,
    pointer: Option<wl_pointer::WlPointer>,
    syncs: usize,
    frames: usize,
    pointer_frames: usize,
    entered: bool,
    keyboard_entered: bool,
    motions: Vec<(f64, f64)>,
    buttons: Vec<wayland_client::WEnum<wl_pointer::ButtonState>>,
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
                "xdg_wm_base" => state.shell = Some(registry.bind(name, 1, qh, ())),
                "wl_seat" => {
                    let seat: wl_seat::WlSeat = registry.bind(name, version.min(5), qh, ());
                    state.pointer = Some(seat.get_pointer(qh, ()));
                    seat.get_keyboard(qh, ());
                }
                _ => {}
            }
        }
    }
}

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
            wl_pointer::Event::Enter { .. } => state.entered = true,
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.motions.push((surface_x, surface_y)),
            wl_pointer::Event::Button { state: button, .. } => state.buttons.push(button),
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
        }
    }
}

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
delegate_noop!(TestClient: ignore wl_shm::WlShm);
delegate_noop!(TestClient: ignore wl_shm_pool::WlShmPool);
delegate_noop!(TestClient: ignore wl_buffer::WlBuffer);
delegate_noop!(TestClient: ignore wl_seat::WlSeat);
delegate_noop!(TestClient: ignore xdg_wm_base::XdgWmBase);
delegate_noop!(TestClient: ignore xdg_toplevel::XdgToplevel);

fn pump(
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
    compositor.flush_clients();
    while client.syncs < target {
        queue.blocking_dispatch(client).unwrap();
    }
}

#[test]
fn mapped_app_receives_focus_frames_and_controller_pointer_events() {
    use std::os::fd::AsFd;

    let mut display = Display::<Compositor>::new().unwrap();
    let (sender, _receiver) = bridge::panel_channel();
    let mut compositor = Compositor::new(display.handle(), sender);
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display
        .handle()
        .insert_client(server_socket, Arc::new(ClientState::default()))
        .unwrap();
    let connection = Connection::from_socket(client_socket).unwrap();
    let mut queue = connection.new_event_queue::<TestClient>();
    let qh = queue.handle();
    let mut client = TestClient::default();
    let _registry = connection.display().get_registry(&qh, ());
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    let surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
    let xdg_surface = client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );

    let mut pixels = tempfile::tempfile().unwrap();
    pixels.write_all(&[255; 100 * 50 * 4]).unwrap();
    let pool = client
        .shm
        .as_ref()
        .unwrap()
        .create_pool(pixels.as_fd(), 100 * 50 * 4, &qh, ());
    let buffer = pool.create_buffer(0, 100, 50, 100 * 4, wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, 100, 50);
    surface.frame(&qh, true);
    surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    assert_eq!(
        client.frames, 1,
        "capturing a panel must unblock client redraws"
    );
    assert!(
        client.keyboard_entered,
        "the initial active app must receive keyboard focus"
    );
    assert!(
        compositor.panels[0].frame.as_ref().unwrap().bytes[..4]
            .iter()
            .all(|byte| *byte == 255)
    );

    for (step, x) in [0.0, 0.1].into_iter().enumerate() {
        assert!(compositor.dispatch_ray(
            Ray3 {
                origin: Vec3::new(x, 0.0, 0.0),
                direction: Vec3::NEG_Z
            },
            step as u32
        ));
        compositor.flush_clients();
        while client.pointer_frames < step + 1 {
            queue.blocking_dispatch(&mut client).unwrap();
        }
    }
    assert!(client.entered);
    assert_eq!(client.motions.last(), Some(&(60.0, 25.0)));
    for (step, pressed) in [true, false].into_iter().enumerate() {
        compositor.dispatch_button(pressed, 10 + step as u32);
        compositor.flush_clients();
        while client.pointer_frames < step + 3 {
            queue.blocking_dispatch(&mut client).unwrap();
        }
    }
    assert_eq!(
        client.buttons,
        vec![
            wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
            wayland_client::WEnum::Value(wl_pointer::ButtonState::Released),
        ]
    );
    surface.frame(&qh, true);
    surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    assert_eq!(client.frames, 2, "subsequent redraws must also complete");
}
