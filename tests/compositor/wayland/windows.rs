use super::super::client::pump;
use super::fixture::WaylandApp;
use crate::Ray3;
use glam::Vec3;
use std::{io::Write, os::fd::AsFd};
use wayland_client::protocol::wl_shm;

#[test]
fn pointer_coordinates_match_logical_content_pixels() {
    let mut app = super::fixture::WaylandApp::new(None);
    let geometry = app.compositor.panels[0].geometry.unwrap();
    let ray_for_point = |x: f32, y: f32| Ray3 {
        origin: geometry.pose.center
            + geometry.pose.orientation()
                * Vec3::new(
                    (x / 100.0 - 0.5) * geometry.pose.width_m,
                    (0.5 - y / 50.0) * geometry.pose.width_m * 0.5,
                    1.0,
                ),
        direction: geometry.pose.orientation() * Vec3::NEG_Z,
    };
    for (x, y) in [
        (10.0, 10.0),
        (90.0, 10.0),
        (10.0, 40.0),
        (90.0, 40.0),
        (50.0, 25.0),
    ] {
        assert!(app.compositor.dispatch_ray(ray_for_point(x, y), 1));
        pump(
            &mut app.display,
            &mut app.compositor,
            &mut app.queue,
            &mut app.client,
            &app.connection,
        );
        let &(actual_x, actual_y) = app.client.motions.last().unwrap();
        assert!(
            (actual_x - x as f64).abs() <= 1.0 / 256.0,
            "expected x={x}, got {actual_x}"
        );
        assert!(
            (actual_y - y as f64).abs() <= 1.0 / 256.0,
            "expected y={y}, got {actual_y}"
        );
    }
    assert!(!app.compositor.dispatch_ray(ray_for_point(-1.0, 25.0), 2));
    assert!(
        app.compositor
            .seat
            .get_pointer()
            .unwrap()
            .current_focus()
            .is_none()
    );
    assert!(app.compositor.dispatch_ray(ray_for_point(10.0, 10.0), 3));
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    let &(actual_x, actual_y) = app.client.motions.last().unwrap();
    assert!((actual_x - 10.0).abs() <= 1.0 / 256.0);
    assert!((actual_y - 10.0).abs() <= 1.0 / 256.0);
}

#[test]
fn panel_bounds_use_wayland_window_geometry() {
    let mut app = super::fixture::WaylandApp::new(None);
    app.xdg_surface.set_window_geometry(10, 5, 80, 40);
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );

    let panel = &app.compositor.panels[0];
    let geometry = panel.geometry.unwrap();
    assert_eq!(geometry.logical_size, (80, 40).into());
    assert_eq!(panel.bounds.loc, (10, 5).into());

    let ray = Ray3 {
        origin: geometry.pose.center + geometry.pose.orientation() * Vec3::new(0.0, 0.0, 1.0),
        direction: geometry.pose.orientation() * Vec3::NEG_Z,
    };
    assert!(app.compositor.dispatch_ray(ray, 1));
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(app.client.motions.last(), Some(&(50.0, 25.0)));
}

#[test]
fn resize_panel_uses_wayland_window_geometry_size() {
    let mut app = super::fixture::WaylandApp::new(None);
    let panel_id = app.compositor.panels[0].id;
    for (geometry, requested, expected) in [
        (None, (120, 70), (120, 70)),
        (Some((10, 5, 80, 40)), (100, 50), (100, 50)),
        (Some((10, 5, 80, 40)), (120, 70), (120, 70)),
        (Some((10, 5, 80, 40)), (130, 80), (130, 80)),
        (Some((10, 5, 80, 40)), (1, 1), (1, 1)),
    ] {
        if let Some((x, y, width, height)) = geometry {
            app.xdg_surface.set_window_geometry(x, y, width, height);
            app.surface.commit();
        }
        pump(
            &mut app.display,
            &mut app.compositor,
            &mut app.queue,
            &mut app.client,
            &app.connection,
        );
        app.compositor.handle_xr_input(crate::XrInput::ResizePanel {
            panel_id,
            width: requested.0,
            height: requested.1,
            anchor: None,
        });
        pump(
            &mut app.display,
            &mut app.compositor,
            &mut app.queue,
            &mut app.client,
            &app.connection,
        );
        assert_eq!(app.client.toplevel_configures.last(), Some(&expected));
    }
}

pub(super) fn exercise(app: &mut WaylandApp) {
    let WaylandApp {
        display,
        receiver,
        compositor,
        connection,
        queue,
        qh,
        client,
        surface,
        vulkan,
        ..
    } = app;
    receiver.drain();
    let first_pose = compositor.panels[0].pose;
    let first_id = compositor.panels[0].id;
    let second_surface = client.compositor.as_ref().unwrap().create_surface(qh, ());
    let second_xdg = client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&second_surface, qh, ());
    let second_toplevel = second_xdg.get_toplevel(qh, ());
    second_surface.commit();
    pump(display, compositor, queue, client, connection);

    let mut green_file = tempfile::tempfile().unwrap();
    green_file
        .write_all(&[0, 255, 0, 255].repeat(1600 * 800))
        .unwrap();
    let green_pool =
        client
            .shm
            .as_ref()
            .unwrap()
            .create_pool(green_file.as_fd(), 1600 * 800 * 4, qh, ());
    let green_buffer =
        green_pool.create_buffer(0, 800, 400, 800 * 4, wl_shm::Format::Argb8888, qh, ());
    second_surface.attach(Some(&green_buffer), 0, 0);
    second_surface.damage(0, 0, 800, 400);
    second_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(compositor.panels[0].pose, first_pose);
    assert_eq!(compositor.panels[0].geometry.unwrap().pose, first_pose);
    let second_id = compositor.panels[1].id;
    let second_pose = compositor.panels[1].pose;
    let second_server_surface = compositor.panels[1].surface.wl_surface().clone();

    let check_green_frame = |expected_size: (i32, i32)| {
        if let Some(vulkan) = vulkan.as_ref() {
            let crate::PanelUpdate::GpuFrame {
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
            let shared = crate::gpu::SharedImage::import(
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
    pump(display, compositor, queue, client, connection);
    client.input_events.clear();
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
    pump(display, compositor, queue, client, connection);
    assert_eq!(client.motions.last(), Some(&(400.0, 200.0)));
    let focus_event = client
        .input_events
        .iter()
        .position(|event| *event == "keyboard-enter")
        .unwrap();
    let button_event = client
        .input_events
        .iter()
        .position(|event| *event == "button-press")
        .unwrap();
    assert!(
        focus_event < button_event,
        "window must receive keyboard focus before its activation click: {:?}",
        client.input_events
    );

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
        green_pool.create_buffer(0, 1600, 800, 1600 * 4, wl_shm::Format::Argb8888, qh, ());
    second_surface.attach(Some(&resized_buffer), 0, 0);
    second_surface.damage(0, 0, 1600, 800);
    second_surface.commit();
    pump(display, compositor, queue, client, connection);
    check_green_frame((1600, 800));
    assert_eq!(compositor.panels[1].geometry.unwrap().pose, second_pose);

    second_surface.attach(None, 0, 0);
    second_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert!(
        matches!(receiver.try_recv().unwrap(), crate::PanelUpdate::Removed { panel_id } if panel_id == second_id)
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
    pump(display, compositor, queue, client, connection);
    second_surface.attach(Some(&green_buffer), 0, 0);
    second_surface.damage(0, 0, 800, 400);
    second_surface.commit();
    pump(display, compositor, queue, client, connection);
    check_green_frame((800, 400));
    assert_eq!(compositor.panels[1].geometry.unwrap().pose, second_pose);

    for _ in 0..8 {
        surface.commit();
        second_surface.commit();
    }
    pump(display, compositor, queue, client, connection);
    let updates = receiver.drain();
    if let Some(vulkan) = vulkan.as_ref() {
        assert_eq!(
            updates.len(),
            2,
            "retain one latest image per window, not a queue of old frames"
        );
        for update in updates {
            let crate::PanelUpdate::GpuFrame {
                panel_id, dmabuf, ..
            } = update
            else {
                panic!("mapped windows must remain independent GPU images");
            };
            let shared = crate::gpu::SharedImage::import(
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
    pump(display, compositor, queue, client, connection);
    assert!(
        matches!(receiver.try_recv().unwrap(), crate::PanelUpdate::Removed { panel_id } if panel_id == second_id)
    );
    assert_eq!(compositor.panels.len(), 1);
    assert_eq!(compositor.active_panel, Some(first_id));
    assert_eq!(compositor.panels[0].pose, first_pose);
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: first_pose.center.normalize()
        },
        25
    ));
    compositor.dispatch_button(true, 26);
    compositor.dispatch_button(false, 27);
    assert_eq!(compositor.active_panel, Some(first_id));

    let replacement_surface = client.compositor.as_ref().unwrap().create_surface(qh, ());
    let replacement_xdg =
        client
            .shell
            .as_ref()
            .unwrap()
            .get_xdg_surface(&replacement_surface, qh, ());
    let replacement_toplevel = replacement_xdg.get_toplevel(qh, ());
    replacement_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(
        compositor.panels[1].pose, second_pose,
        "reuse freed placement without moving other windows"
    );
    replacement_toplevel.destroy();
    replacement_xdg.destroy();
    replacement_surface.destroy();
    pump(display, compositor, queue, client, connection);
}
