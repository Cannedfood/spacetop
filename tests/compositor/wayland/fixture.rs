use super::super::client::{TestClient, pump};
use crate::{ClientState, Compositor, Display, bridge};
use std::{io::Write, os::fd::AsFd, os::unix::net::UnixStream, sync::Arc};
use wayland_client::{
    Connection, EventQueue, QueueHandle,
    protocol::{wl_shm, wl_surface},
};
use wayland_protocols::xdg::shell::client::xdg_surface;

pub(super) struct WaylandApp {
    pub(super) display: Display<Compositor>,
    pub(super) receiver: bridge::PanelReceiver,
    pub(super) compositor: Compositor,
    pub(super) connection: Connection,
    pub(super) queue: EventQueue<TestClient>,
    pub(super) qh: QueueHandle<TestClient>,
    pub(super) client: TestClient,
    pub(super) surface: wl_surface::WlSurface,
    pub(super) xdg_surface: xdg_surface::XdgSurface,
    pub(super) vulkan: Option<crate::gpu::test_support::Vulkan>,
}

impl WaylandApp {
    pub(super) fn new(vulkan: Option<crate::gpu::test_support::Vulkan>) -> Self {
        let mut display = Display::<Compositor>::new().unwrap();
        let (sender, receiver) = bridge::panel_channel();
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
        let mut content = vec![0_u8; 100 * 50 * 4];
        for (index, pixel) in content.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *pixel = if index / 100 < 25 {
                [0, 0, 255, 255]
            } else {
                [255, 0, 0, 255]
            };
        }
        pixels.write_all(&content).unwrap();
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

        Self {
            display,
            receiver,
            compositor,
            connection,
            queue,
            qh,
            client,
            surface,
            xdg_surface,
            vulkan,
        }
    }
}
