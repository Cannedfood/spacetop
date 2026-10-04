mod basic;
mod fixture;
mod menus;
mod surfaces;
mod windows;

#[test]
#[ignore = "requires a DRM render node and Vulkan DMA-BUF import support"]
fn gpu_capture_preserves_moved_and_resized_panel_size() {
    use std::{io::Write, os::fd::AsFd};
    use wayland_client::protocol::wl_shm;

    let vulkan = crate::gpu::test_support::Vulkan::new().unwrap();
    let mut app = fixture::WaylandApp::new(None);
    app.xdg_surface.set_window_geometry(10, 5, 80, 40);
    app.surface.commit();
    super::client::pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    app.compositor.configure_gpu(&vulkan.render_node).unwrap();
    app.compositor.capture_panel(0).unwrap();
    let panel_id = app.compositor.panels[0].id;
    let mut pose = app.compositor.panels[0].geometry.unwrap().pose;
    pose.center.x += 0.4;
    pose.width_m *= 1.2;
    app.compositor
        .handle_xr_input(crate::XrInput::MovePanel { panel_id, pose });
    app.compositor.capture_panel(0).unwrap();
    assert_eq!(app.compositor.panels[0].geometry.unwrap().pose, pose);

    let geometry = app.compositor.panels[0].geometry.unwrap();
    assert_eq!(geometry.logical_size, (80, 40).into());
    let pose = geometry.resized_pose_from_edges(
        (100, 60).into(),
        [false, true, false, true],
        app.compositor.window_pixels_per_degree,
    );
    app.compositor.handle_xr_input(crate::XrInput::ResizePanel {
        panel_id,
        width: 120,
        height: 70,
        anchor: Some((geometry, [false, true, false, true])),
    });
    assert_eq!(app.compositor.panels[0].geometry, Some(geometry));
    super::client::pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(app.client.toplevel_configures.last(), Some(&(120, 70)));
    let mut pixels = tempfile::tempfile().unwrap();
    pixels
        .write_all(&[0, 255, 0, 255].repeat(140 * 90))
        .unwrap();
    let pool =
        app.client
            .shm
            .as_ref()
            .unwrap()
            .create_pool(pixels.as_fd(), 140 * 90 * 4, &app.qh, ());
    let buffer = pool.create_buffer(0, 120, 70, 120 * 4, wl_shm::Format::Argb8888, &app.qh, ());
    app.xdg_surface.set_window_geometry(10, 5, 100, 60);
    app.surface.attach(Some(&buffer), 0, 0);
    app.surface.damage(0, 0, 120, 70);
    app.surface.commit();
    super::client::pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].geometry.unwrap().logical_size,
        (100, 60).into()
    );
    assert_eq!(app.compositor.panels[0].geometry.unwrap().pose, pose);

    app.compositor.handle_xr_input(crate::XrInput::ResizePanel {
        panel_id,
        width: 140,
        height: 90,
        anchor: Some((geometry, [false, true, false, true])),
    });
    assert_eq!(app.compositor.panels[0].geometry.unwrap().pose, pose);
    for (width, height) in [(120, 70), (125, 75), (130, 80)] {
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Argb8888,
            &app.qh,
            (),
        );
        app.xdg_surface
            .set_window_geometry(10, 5, width - 20, height - 10);
        app.surface.attach(Some(&buffer), 0, 0);
        app.surface.damage(0, 0, width, height);
        app.surface.commit();
        super::client::pump(
            &mut app.display,
            &mut app.compositor,
            &mut app.queue,
            &mut app.client,
            &app.connection,
        );
        let actual = app.compositor.panels[0].geometry.unwrap();
        assert_eq!(actual.logical_size, (width - 20, height - 10).into());
        assert_eq!(
            actual.pose,
            geometry.resized_pose_from_edges(
                actual.logical_size,
                [false, true, false, true],
                app.compositor.window_pixels_per_degree,
            )
        );
    }
}

#[test]
#[ignore = "requires a DRM render node and Vulkan DMA-BUF import support"]
fn gpu_frame_ticks_coalesce_commits_and_skip_clean_panels() {
    let vulkan = crate::gpu::test_support::Vulkan::new().unwrap();
    let mut app = fixture::WaylandApp::new(None);
    app.compositor.configure_gpu(&vulkan.render_node).unwrap();
    for _ in 0..10 {
        app.surface.damage(0, 0, 100, 50);
        app.surface.frame(&app.qh, true);
        app.surface.commit();
    }
    app.connection.flush().unwrap();
    app.display.dispatch_clients(&mut app.compositor).unwrap();
    let event_loop =
        smithay::reexports::calloop::EventLoop::<crate::Compositor>::try_new().unwrap();
    let signal = event_loop.get_signal();
    app.compositor.finish_dispatch(&signal);
    assert!(
        app.receiver.try_recv().is_err(),
        "commits must not capture before an XR tick"
    );
    assert_eq!(app.compositor.dirty_panels.len(), 1);
    app.compositor.handle_xr_input(crate::XrInput::FrameTick);
    app.compositor.finish_dispatch(&signal);
    assert_eq!(app.receiver.drain().len(), 1);
    assert!(app.compositor.dirty_panels.is_empty());
    super::client::pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(app.client.frames, 11);
    app.compositor.handle_xr_input(crate::XrInput::FrameTick);
    app.compositor.finish_dispatch(&signal);
    assert!(
        app.receiver.try_recv().is_err(),
        "clean panels must not be recaptured"
    );
}

#[test]
#[ignore = "requires zenity (GTK) on PATH"]
fn gtk_app_maps_a_window() {
    use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
    use std::{
        io::Read,
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::{Command, Stdio},
        sync::Arc,
        time::{Duration, Instant},
    };

    let mut app = fixture::WaylandApp::new(None);
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    app.display
        .handle()
        .insert_client(server_socket, Arc::new(crate::ClientState::default()))
        .unwrap();
    let mut event_loop = EventLoop::<crate::Compositor>::try_new().unwrap();
    event_loop
        .handle()
        .insert_source(
            Generic::new(app.display, Interest::READ, Mode::Level),
            |_, display, compositor| {
                unsafe {
                    display.get_mut().dispatch_clients(compositor)?;
                }
                Ok(PostAction::Continue)
            },
        )
        .unwrap();
    let mut child = Command::new("zenity")
        .args(["--info", "--text=Spacetop GTK compatibility test"])
        .env("GDK_BACKEND", "wayland")
        .env("GSK_RENDERER", "cairo")
        .env("WAYLAND_SOCKET", "0")
        .env_remove("DISPLAY")
        .stdin(Stdio::from(OwnedFd::from(client_socket)))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut mapped = false;
    while Instant::now() < deadline {
        event_loop
            .dispatch(Duration::from_millis(10), &mut app.compositor)
            .unwrap();
        app.compositor.flush_clients();
        mapped = app
            .compositor
            .panels
            .iter()
            .skip(1)
            .any(|panel| panel.geometry.is_some());
        if mapped || child.try_wait().unwrap().is_some() {
            break;
        }
    }
    let _ = child.kill();
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        mapped,
        "GTK client did not map a window ({status}):\n{stderr}"
    );
}

#[test]
fn app_waits_for_gpu_setup_and_receives_pointer_events() {
    exercise_wayland_app(None);
}

#[test]
#[ignore = "requires a DRM render node and Vulkan DMA-BUF import support"]
fn gpu_shared_app_preserves_pixels_focus_and_input() {
    exercise_wayland_app(Some(crate::gpu::test_support::Vulkan::new().unwrap()));
}

fn exercise_wayland_app(vulkan: Option<crate::gpu::test_support::Vulkan>) {
    let mut app = fixture::WaylandApp::new(vulkan);
    basic::exercise(&mut app);
    menus::exercise(&mut app);
    surfaces::exercise(&mut app);
    windows::exercise(&mut app);
}
