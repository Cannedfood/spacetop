use std::{io::Write, os::unix::net::UnixStream, sync::Arc};

use glam::Vec3;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat,
        wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use super::{ClientState, Compositor, Display, Ray3, bridge};

#[derive(Default)]
struct TestClient {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    shell: Option<xdg_wm_base::XdgWmBase>,
    pointer: Option<wl_pointer::WlPointer>,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
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
                "zwp_linux_dmabuf_v1" => state.dmabuf = Some(registry.bind(name, 3, qh, ())),
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
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => {
                state.entered = true;
                state.motions.push((surface_x, surface_y));
            }
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
delegate_noop!(TestClient: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
delegate_noop!(TestClient: ignore zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1);

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
fn app_waits_for_gpu_setup_and_receives_pointer_events() {
    exercise_wayland_app(None);
}

#[test]
fn gpu_and_xr_failures_are_fatal() {
    let directory = tempfile::tempdir().unwrap();
    for command in [
        super::XrInput::GpuDevice {
            limits: crate::panel::PanelLimits::default(),
            render_node: directory.path().join("missing-render-node"),
        },
        super::XrInput::FatalError {
            message: "mandatory GPU DMA-BUF import failed".into(),
        },
    ] {
        let display = Display::<Compositor>::new().unwrap();
        let (sender, receiver) = bridge::panel_channel();
        let mut compositor = Compositor::new(display.handle(), sender);
        compositor.handle_xr_input(command);
        assert!(compositor.fatal_error.is_some());
        assert!(compositor.gpu_renderer.is_none());
        assert!(receiver.try_recv().is_err());
        let mut event_loop =
            smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
        let signal = event_loop.get_signal();
        event_loop
            .run(
                Some(std::time::Duration::ZERO),
                &mut compositor,
                |compositor| compositor.finish_dispatch(&signal),
            )
            .unwrap();
    }
}

#[test]
#[ignore = "requires a DRM render node and Vulkan DMA-BUF import support"]
fn gpu_shared_app_preserves_pixels_focus_and_input() {
    exercise_wayland_app(Some(super::gpu::test_support::Vulkan::new().unwrap()));
}

#[test]
#[ignore = "requires Xwayland and XDG_RUNTIME_DIR"]
fn x11_app_waits_for_gpu_and_receives_input() {
    exercise_x11_app(None);
}

#[test]
#[ignore = "requires Xwayland, XDG_RUNTIME_DIR, and Vulkan DMA-BUF import support"]
fn gpu_shared_x11_app_preserves_pixels_and_input() {
    exercise_x11_app(Some(super::gpu::test_support::Vulkan::new().unwrap()));
}

fn exercise_x11_app(vulkan: Option<super::gpu::test_support::Vulkan>) {
    use smithay::reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
        x11rb::{
            connection::Connection as _,
            protocol::{
                Event,
                xproto::{ConnectionExt as _, CreateWindowAux, EventMask, WindowClass},
            },
        },
    };
    use std::time::{Duration, Instant};

    fn until(
        event_loop: &mut EventLoop<'_, Compositor>,
        compositor: &mut Compositor,
        mut condition: impl FnMut(&Compositor) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition(compositor) {
            assert!(Instant::now() < deadline, "X11 integration test timed out");
            event_loop
                .dispatch(Duration::from_millis(10), compositor)
                .unwrap();
            compositor.flush_clients();
            assert!(
                compositor.fatal_error.is_none(),
                "{:?}",
                compositor.fatal_error
            );
        }
    }

    let mut event_loop = EventLoop::<Compositor>::try_new().unwrap();
    let display = Display::<Compositor>::new().unwrap();
    let (sender, receiver) = bridge::panel_channel();
    let mut compositor = Compositor::new(display.handle(), sender);
    if let Some(vulkan) = &vulkan {
        compositor.configure_gpu(&vulkan.render_node).unwrap();
    }
    let display_number = super::x11::start(
        &display.handle(),
        event_loop.handle(),
        "spacetop-test".into(),
        None,
    )
    .unwrap()
    .expect("Xwayland is installed");
    let display_name = format!(":{display_number}");
    event_loop
        .handle()
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, compositor| {
                unsafe {
                    display.get_mut().dispatch_clients(compositor)?;
                }
                Ok(PostAction::Continue)
            },
        )
        .unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.xwm.is_some()
    });
    let (connection, screen_number) =
        smithay::reexports::x11rb::connect(Some(&display_name)).unwrap();
    let screen = &connection.setup().roots[screen_number];
    let window = connection.generate_id().unwrap();
    connection
        .create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            100,
            50,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .background_pixel(0x00ff0000)
                .event_mask(
                    EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::POINTER_MOTION
                        | EventMask::FOCUS_CHANGE,
                ),
        )
        .unwrap();
    connection.map_window(window).unwrap();
    connection.flush().unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.panels.len() == 1 && state.panels[0].geometry.is_some()
    });
    assert_eq!(
        compositor.panels[0].geometry.unwrap().logical_size,
        (100, 50).into()
    );
    let panel_id = compositor.panels[0].id;
    let pose = compositor.panels[0].pose;
    if let Some(vulkan) = &vulkan {
        let super::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("expected a captured X11 frame");
        };
        let shared = super::gpu::SharedImage::import(
            &vulkan.instance,
            &vulkan.device,
            vulkan.physical_device,
            dmabuf,
        )
        .unwrap();
        let pixels = vulkan.readback(&shared, None).unwrap();
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [255, 0, 0, 255])
        );
    } else {
        assert!(receiver.try_recv().is_err());
    }
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: pose.center.normalize()
        },
        10
    ));
    compositor.dispatch_button(true, 11);
    compositor.dispatch_button(false, 12);
    compositor.flush_clients();
    let mut pressed = false;
    let mut released = false;
    let mut moved = false;
    until(&mut event_loop, &mut compositor, |_| {
        while let Some(event) = connection.poll_for_event().unwrap() {
            match event {
                Event::ButtonPress(event) => {
                    assert_eq!(event.detail, 1);
                    pressed = true;
                }
                Event::ButtonRelease(event) => {
                    assert_eq!(event.detail, 1);
                    released = true;
                }
                Event::MotionNotify(event) => {
                    assert_eq!((event.event_x, event.event_y), (50, 25));
                    moved = true;
                }
                _ => {}
            }
        }
        pressed && released && moved
    });
    assert_eq!(
        connection.get_input_focus().unwrap().reply().unwrap().focus,
        window
    );

    if vulkan.is_none() {
        connection.destroy_window(window).unwrap();
        connection.flush().unwrap();
        until(&mut event_loop, &mut compositor, |state| {
            state.panels.is_empty()
        });
        assert!(compositor.active_panel.is_none());
        assert!(
            matches!(receiver.try_recv().unwrap(), super::PanelUpdate::Removed { panel_id: removed } if removed == panel_id)
        );
        return;
    }

    connection
        .configure_window(
            window,
            &smithay::reexports::x11rb::protocol::xproto::ConfigureWindowAux::new()
                .width(160)
                .height(80),
        )
        .unwrap();
    connection.flush().unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.panels[0].geometry.unwrap().logical_size == (160, 80).into()
    });
    assert_eq!(compositor.panels[0].pose, pose);
    assert_eq!(compositor.panels[0].id, panel_id);
    connection.unmap_window(window).unwrap();
    connection.flush().unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.panels.is_empty()
    });
    assert!(compositor.active_panel.is_none());
    assert!(
        compositor
            .seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .is_none()
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), super::PanelUpdate::Removed { panel_id: removed } if removed == panel_id)
    );
    connection.map_window(window).unwrap();
    connection.flush().unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.panels.len() == 1 && state.panels[0].geometry.is_some()
    });
    assert_eq!(compositor.panels[0].pose, pose);
    connection.destroy_window(window).unwrap();
    connection.flush().unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.panels.is_empty()
    });
    assert!(compositor.active_panel.is_none());
}

fn exercise_wayland_app(vulkan: Option<super::gpu::test_support::Vulkan>) {
    use std::os::fd::AsFd;

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
    assert_eq!(
        client.frames, 0,
        "redraws must wait until GPU setup completes"
    );
    assert!(receiver.try_recv().is_err());
    if let Some(vulkan) = vulkan.as_ref() {
        compositor.configure_gpu(&vulkan.render_node).unwrap();
        pump(
            &mut display,
            &mut compositor,
            &mut queue,
            &mut client,
            &connection,
        );
        assert_eq!(client.frames, 1, "GPU setup must resume pending redraws");
    }
    assert!(
        client.keyboard_entered,
        "the initial active app must receive keyboard focus"
    );
    if let Some(vulkan) = vulkan.as_ref() {
        let super::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("GPU capture must publish a DMA-BUF, not a CPU snapshot");
        };
        let shared = super::gpu::SharedImage::import(
            &vulkan.instance,
            &vulkan.device,
            vulkan.physical_device,
            dmabuf,
        )
        .unwrap();
        let pixels = vulkan.readback(&shared, None).unwrap();
        assert_eq!(&pixels[..4], &[255, 0, 0, 255], "top row must remain red");
        assert_eq!(
            &pixels[49 * 100 * 4..49 * 100 * 4 + 4],
            &[0, 0, 255, 255],
            "bottom row must remain blue"
        );
        let cursor_pixels = vulkan.readback(&shared, Some((50, 25))).unwrap();
        assert_eq!(
            &cursor_pixels[(25 * 100 + 50) * 4..(25 * 100 + 50) * 4 + 4],
            &[255, 245, 0, 255]
        );
        assert_eq!(
            &cursor_pixels[(30 * 100 + 30) * 4..(30 * 100 + 30) * 4 + 4],
            &[0, 0, 255, 255]
        );

        let params = client
            .dmabuf
            .as_ref()
            .expect("GPU compositor must advertise linux-dmabuf")
            .create_params(&qh, ());
        params.add(
            shared.dmabuf.handles().next().unwrap(),
            0,
            shared.dmabuf.offsets().next().unwrap(),
            shared.dmabuf.strides().next().unwrap(),
            0,
            0,
        );
        let gpu_buffer = params.create_immed(
            100,
            50,
            smithay::backend::allocator::Fourcc::Abgr8888 as u32,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &qh,
            (),
        );
        surface.attach(Some(&gpu_buffer), 0, 0);
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
        assert_eq!(client.frames, 2);
        let super::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("GPU-backed app buffers must remain on the shared path");
        };
        let imported = super::gpu::SharedImage::import(
            &vulkan.instance,
            &vulkan.device,
            vulkan.physical_device,
            dmabuf,
        )
        .unwrap();
        assert_eq!(vulkan.readback(&imported, None).unwrap(), pixels);
    } else {
        assert!(
            receiver.try_recv().is_err(),
            "rendering must wait for GPU setup"
        );
    }

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
    assert_eq!(
        client.frames,
        if vulkan.is_some() { 3 } else { 0 },
        "subsequent redraws must also complete"
    );

    receiver.drain();
    let first_pose = compositor.panels[0].pose;
    let first_id = compositor.panels[0].id;
    let second_surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
    let second_xdg = client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&second_surface, &qh, ());
    let second_toplevel = second_xdg.get_toplevel(&qh, ());
    second_surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );

    let mut green_file = tempfile::tempfile().unwrap();
    green_file
        .write_all(&[0, 255, 0, 255].repeat(1600 * 800))
        .unwrap();
    let green_pool =
        client
            .shm
            .as_ref()
            .unwrap()
            .create_pool(green_file.as_fd(), 1600 * 800 * 4, &qh, ());
    let green_buffer =
        green_pool.create_buffer(0, 800, 400, 800 * 4, wl_shm::Format::Argb8888, &qh, ());
    second_surface.attach(Some(&green_buffer), 0, 0);
    second_surface.damage(0, 0, 800, 400);
    second_surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    assert_eq!(compositor.panels[0].pose, first_pose);
    assert_eq!(compositor.panels[0].geometry.unwrap().pose, first_pose);
    let second_id = compositor.panels[1].id;
    let second_pose = compositor.panels[1].pose;
    let second_server_surface = compositor.panels[1].surface.wl_surface().clone();

    let check_green_frame = |expected_size: (i32, i32)| {
        if let Some(vulkan) = &vulkan {
            let super::PanelUpdate::GpuFrame {
                panel_id,
                dmabuf,
                geometry,
            } = receiver.try_recv().unwrap()
            else {
                panic!("mapped window must publish its own GPU image");
            };
            assert_eq!(panel_id, second_id);
            assert_eq!(geometry.pose, second_pose);
            assert_eq!(geometry.logical_size, expected_size.into());
            let shared = super::gpu::SharedImage::import(
                &vulkan.instance,
                &vulkan.device,
                vulkan.physical_device,
                dmabuf,
            )
            .unwrap();
            assert_eq!(
                smithay::backend::allocator::Buffer::size(&shared.dmabuf),
                expected_size.into()
            );
            let output = vulkan.readback(&shared, None).unwrap();
            assert!(
                output
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| *pixel == [0, 255, 0, 255])
            );
        } else {
            assert!(receiver.try_recv().is_err());
        }
    };
    check_green_frame((800, 400));
    if vulkan.is_some() {
        let layer_limit = compositor.panel_limits.max_layers;
        compositor.panel_limits.max_layers = 1;
        assert!(
            compositor
                .capture_panel(1)
                .unwrap_err()
                .to_string()
                .contains("at most 1 mapped window")
        );
        compositor.panel_limits.max_layers = layer_limit;
    }
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: second_pose.center.normalize()
        },
        20
    ));
    assert_eq!(
        compositor.seat.get_pointer().unwrap().current_focus(),
        Some(second_server_surface.clone())
    );
    compositor.dispatch_button(true, 21);
    compositor.dispatch_button(false, 22);
    assert_eq!(compositor.active_panel, Some(second_id));
    assert_eq!(
        compositor
            .seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .map(|surface| surface.wl_surface().clone()),
        Some(second_server_surface.clone())
    );
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    assert_eq!(client.motions.last(), Some(&(400.0, 200.0)));

    compositor.panels[1].geometry.as_mut().unwrap().pose.center = Vec3::new(0.0, 0.0, -0.8);
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z
        },
        23
    ));
    assert_eq!(
        compositor.seat.get_pointer().unwrap().current_focus(),
        Some(second_server_surface.clone()),
        "nearest hit must win even when the oldest window overlaps"
    );

    let resized_buffer =
        green_pool.create_buffer(0, 1600, 800, 1600 * 4, wl_shm::Format::Argb8888, &qh, ());
    second_surface.attach(Some(&resized_buffer), 0, 0);
    second_surface.damage(0, 0, 1600, 800);
    second_surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    check_green_frame((1600, 800));
    assert_eq!(compositor.panels[1].geometry.unwrap().pose, second_pose);

    second_surface.attach(None, 0, 0);
    second_surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), super::PanelUpdate::Removed { panel_id } if panel_id == second_id)
    );
    assert!(compositor.panels[1].geometry.is_none());
    assert_eq!(compositor.active_panel, Some(first_id));
    assert_eq!(
        compositor
            .seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .map(|surface| surface.wl_surface().clone()),
        Some(compositor.panels[0].surface.wl_surface().clone())
    );
    assert!(!compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: second_pose.center.normalize()
        },
        24
    ));

    second_surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    second_surface.attach(Some(&green_buffer), 0, 0);
    second_surface.damage(0, 0, 800, 400);
    second_surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    check_green_frame((800, 400));
    assert_eq!(compositor.panels[1].geometry.unwrap().pose, second_pose);

    for _ in 0..8 {
        surface.commit();
        second_surface.commit();
    }
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    let updates = receiver.drain();
    if let Some(vulkan) = &vulkan {
        assert_eq!(
            updates.len(),
            2,
            "retain one latest image per window, not a queue of old frames"
        );
        for update in updates {
            let super::PanelUpdate::GpuFrame {
                panel_id, dmabuf, ..
            } = update
            else {
                panic!("mapped windows must remain independent GPU images");
            };
            let shared = super::gpu::SharedImage::import(
                &vulkan.instance,
                &vulkan.device,
                vulkan.physical_device,
                dmabuf,
            )
            .unwrap();
            let output = vulkan.readback(&shared, None).unwrap();
            assert_eq!(
                &output[..4],
                if panel_id == first_id {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 255, 0, 255]
                }
            );
        }
    } else {
        assert!(updates.is_empty());
    }
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: second_pose.center.normalize()
        },
        25
    ));
    compositor.dispatch_button(true, 26);
    compositor.dispatch_button(false, 27);
    assert_eq!(compositor.active_panel, Some(second_id));
    second_toplevel.destroy();
    second_xdg.destroy();
    second_surface.destroy();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    assert!(
        matches!(receiver.try_recv().unwrap(), super::PanelUpdate::Removed { panel_id } if panel_id == second_id)
    );
    assert_eq!(compositor.panels.len(), 1);
    assert_eq!(compositor.active_panel, Some(first_id));
    assert_eq!(compositor.panels[0].pose, first_pose);
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z
        },
        25
    ));
    compositor.dispatch_button(true, 26);
    compositor.dispatch_button(false, 27);
    assert_eq!(compositor.active_panel, Some(first_id));

    let replacement_surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
    let replacement_xdg =
        client
            .shell
            .as_ref()
            .unwrap()
            .get_xdg_surface(&replacement_surface, &qh, ());
    let replacement_toplevel = replacement_xdg.get_toplevel(&qh, ());
    replacement_surface.commit();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
    assert_eq!(
        compositor.panels[1].pose, second_pose,
        "reuse freed placement without moving other windows"
    );
    replacement_toplevel.destroy();
    replacement_xdg.destroy();
    replacement_surface.destroy();
    pump(
        &mut display,
        &mut compositor,
        &mut queue,
        &mut client,
        &connection,
    );
}
