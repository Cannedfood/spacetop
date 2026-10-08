use super::super::client::pump;
use super::fixture::WaylandApp;
use crate::Ray3;
use glam::Vec3;
use std::{io::Write, os::fd::AsFd};
use wayland_client::protocol::wl_shm;

fn has_maximized_state(states: &[u8]) -> bool {
    has_toplevel_state(states, 1)
}

fn has_fullscreen_state(states: &[u8]) -> bool {
    has_toplevel_state(states, 2)
}

fn has_toplevel_state(states: &[u8], expected_state: u32) -> bool {
    states
        .as_chunks::<4>()
        .0
        .iter()
        .any(|state| u32::from_ne_bytes(*state) == expected_state)
}

#[test]
fn mouse_cursor_uses_world_hit_for_unnormalized_ray() {
    let app = WaylandApp::new(None);
    let geometry = app.compositor.panels[0].geometry.unwrap();
    let ray = Ray3 {
        origin: Vec3::ZERO,
        direction: geometry.pose.center * 2.0,
    };
    let cursor = app.compositor.mouse_cursor_pose(ray).unwrap();
    assert!((cursor.center - geometry.pose.center).length() < 1.0e-5);
}

#[test]
fn fullscreen_requests_toggle_compositor_panel_state() {
    let mut app = super::fixture::WaylandApp::new(None);
    let geometry = app.compositor.panels[0].geometry.unwrap();
    let config = &app.compositor.window_config;
    let expected_size = geometry.size_for_angular_bounds(
        geometry.pose.center.length(),
        config.pixels_per_degree,
        config.fullscreen_max_width_degrees,
        config.fullscreen_max_height_degrees,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState::default()
    );

    app.toplevel.set_fullscreen(None);
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState {
            fullscreen: true,
            ..Default::default()
        }
    );
    assert_eq!(app.client.toplevel_configures.last(), Some(&expected_size));

    app.toplevel.unset_fullscreen();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState::default()
    );
    assert_eq!(
        app.client.toplevel_configures.last(),
        Some(&(geometry.logical_size.w, geometry.logical_size.h))
    );
}

#[test]
fn fullscreen_exit_restores_maximized_size_and_distance_but_not_position() {
    use crate::bridge::PanelState;
    let mut app = WaylandApp::new(None);
    app.compositor.request_panel_maximized(0, true).unwrap();
    app.compositor.request_panel_fullscreen(0, true).unwrap();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    let states = app.client.toplevel_states.last().unwrap();
    assert!(has_maximized_state(states));
    assert!(has_fullscreen_state(states));
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert!(app.compositor.panels[0].state.maximized);
    assert!(app.compositor.panels[0].state.fullscreen);
    app.compositor.panels[0].history.save(
        PanelState {
            maximized: true,
            ..Default::default()
        },
        crate::panel::PanelPastState {
            distance: 2.5,
            size: (1200, 800).into(),
        },
    );
    let panel_id = app.compositor.panels[0].id;
    let mut pose = app.compositor.panels[0].geometry.unwrap().pose;
    pose.center = Vec3::new(1.0, 0.5, -1.0);
    app.compositor
        .handle_xr_input(crate::XrInput::MovePanel { panel_id, pose });
    app.toplevel.unset_fullscreen();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        PanelState {
            maximized: true,
            ..Default::default()
        }
    );
    assert_eq!(app.client.toplevel_configures.last(), Some(&(1200, 800)));
    assert!(has_maximized_state(
        app.client.toplevel_states.last().unwrap()
    ));
    assert!(!has_fullscreen_state(
        app.client.toplevel_states.last().unwrap()
    ));
    let restored = app.compositor.panels[0].geometry.unwrap().pose;
    assert!((restored.center.length() - 2.5).abs() < 1.0e-5);
    assert!(restored.center.normalize().dot(pose.center.normalize()) > 0.99999);
    app.compositor.request_panel_maximized(0, false).unwrap();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(app.client.toplevel_configures.last(), Some(&(100, 50)));
}

#[test]
fn xdg_maximize_requests_update_client_state_and_surface_size() {
    let mut app = super::fixture::WaylandApp::new(None);
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState::default()
    );
    let original_size = app.compositor.panels[0].geometry.unwrap().logical_size;

    app.toplevel.set_maximized();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState {
            maximized: true,
            ..Default::default()
        }
    );
    assert!(has_maximized_state(
        app.client.toplevel_states.last().unwrap()
    ));
    assert!(app.client.toplevel_configures.last().unwrap().0 > original_size.w);

    app.toplevel.unset_maximized();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState::default()
    );
    assert!(!has_maximized_state(
        app.client.toplevel_states.last().unwrap()
    ));
    assert_eq!(
        app.client.toplevel_configures.last(),
        Some(&(original_size.w, original_size.h))
    );
}

#[test]
fn xr_maximize_notifies_client_and_restores_surface_size() {
    let mut app = super::fixture::WaylandApp::new(None);
    let panel_id = app.compositor.panels[0].id;
    let original_size = app.compositor.panels[0].geometry.unwrap().logical_size;

    app.compositor
        .handle_xr_input(crate::XrInput::ToggleMaximize { panel_id });
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState {
            maximized: true,
            ..Default::default()
        }
    );
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert!(has_maximized_state(
        app.client.toplevel_states.last().unwrap()
    ));
    let maximized_size = *app.client.toplevel_configures.last().unwrap();
    assert!(maximized_size.0 > original_size.w);
    assert!(maximized_size.1 > original_size.h);
    assert_eq!(
        app.compositor.panels[0].geometry.unwrap().logical_size,
        original_size
    );

    app.compositor
        .handle_xr_input(crate::XrInput::ToggleMaximize { panel_id });
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState::default()
    );
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert!(!has_maximized_state(
        app.client.toplevel_states.last().unwrap()
    ));
    assert_eq!(
        app.client.toplevel_configures.last(),
        Some(&(original_size.w, original_size.h))
    );
    assert_eq!(
        app.compositor.panels[0].geometry.unwrap().logical_size,
        original_size
    );
}

#[test]
fn maximizing_fullscreen_window_preserves_fullscreen() {
    let mut app = super::fixture::WaylandApp::new(None);
    app.toplevel.set_fullscreen(None);
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState {
            fullscreen: true,
            ..Default::default()
        }
    );

    let panel_id = app.compositor.panels[0].id;
    app.compositor
        .handle_xr_input(crate::XrInput::ToggleMaximize { panel_id });
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState {
            maximized: true,
            fullscreen: true,
            ..Default::default()
        }
    );
    let states = app.client.toplevel_states.last().unwrap();
    assert!(has_maximized_state(states));
    assert!(has_fullscreen_state(states));
}

#[test]
fn unmaximizing_fullscreen_window_preserves_fullscreen_and_restores_regular_size() {
    let mut app = WaylandApp::new(None);
    let original_size = app.compositor.panels[0].geometry.unwrap().logical_size;
    app.compositor.request_panel_maximized(0, true).unwrap();
    app.compositor.request_panel_fullscreen(0, true).unwrap();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    let fullscreen_size = *app.client.toplevel_configures.last().unwrap();
    let fullscreen_pose = app.compositor.panels[0].geometry.unwrap().pose;

    app.toplevel.unset_maximized();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    app.surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert!(!app.compositor.panels[0].state.maximized);
    assert!(app.compositor.panels[0].state.fullscreen);
    let states = app.client.toplevel_states.last().unwrap();
    assert!(!has_maximized_state(states));
    assert!(has_fullscreen_state(states));
    assert_eq!(
        app.client.toplevel_configures.last(),
        Some(&fullscreen_size)
    );
    assert_eq!(
        app.compositor.panels[0].geometry.unwrap().pose,
        fullscreen_pose
    );

    app.toplevel.unset_fullscreen();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(
        app.compositor.panels[0].state,
        crate::bridge::PanelState::default()
    );
    assert_eq!(
        app.client.toplevel_configures.last(),
        Some(&(original_size.w, original_size.h))
    );
}

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
fn presented_window_geometry_controls_pointer_coordinates() {
    for state in [
        crate::bridge::PanelState {
            maximized: true,
            ..Default::default()
        },
        crate::bridge::PanelState {
            fullscreen: true,
            ..Default::default()
        },
    ] {
        let mut app = WaylandApp::new(None);
        let panel_id = app.compositor.panels[0].id;
        let committed = app.compositor.panels[0].geometry.unwrap();
        app.compositor.panels[0].state = state;
        let mut presented = committed;
        presented.pose.width_m *= 3.0;
        presented.pose.center += Vec3::new(0.3, 0.2, 0.0);
        app.compositor
            .handle_xr_input(crate::XrInput::PresentedPanels {
                geometries: vec![(panel_id, presented)],
            });

        let local = Vec3::new(presented.pose.width_m * 0.25, 0.0, 1.0);
        let ray = Ray3 {
            origin: presented.pose.center + presented.pose.orientation() * local,
            direction: presented.pose.orientation() * Vec3::NEG_Z,
        };
        assert!(app.compositor.dispatch_ray(ray, 1));
        pump(
            &mut app.display,
            &mut app.compositor,
            &mut app.queue,
            &mut app.client,
            &app.connection,
        );
        let &(actual_x, actual_y) = app.client.motions.last().unwrap();
        assert!((actual_x - 75.0).abs() <= 1.0 / 256.0);
        assert!((actual_y - 25.0).abs() <= 1.0 / 256.0);
        assert_eq!(app.compositor.panels[0].geometry, Some(committed));
        let cursor = app.compositor.mouse_cursor_pose(ray).unwrap();
        assert!(cursor.center.distance(ray.origin + ray.direction) < 1.0e-5);

        app.compositor
            .handle_xr_input(crate::XrInput::PresentedPanels {
                geometries: Vec::new(),
            });
        let ray = Ray3 {
            origin: committed.pose.center + committed.pose.orientation() * Vec3::new(0.0, 0.0, 1.0),
            direction: committed.pose.orientation() * Vec3::NEG_Z,
        };
        assert!(app.compositor.dispatch_ray(ray, 2));
        pump(
            &mut app.display,
            &mut app.compositor,
            &mut app.queue,
            &mut app.client,
            &app.connection,
        );
        let &(actual_x, actual_y) = app.client.motions.last().unwrap();
        assert!((actual_x - 50.0).abs() <= 1.0 / 256.0);
        assert!((actual_y - 25.0).abs() <= 1.0 / 256.0);
    }
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
            pixels_per_degree: None,
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

#[test]
fn close_panel_sends_a_close_request_to_the_wayland_window() {
    let mut app = super::fixture::WaylandApp::new(None);
    let panel_id = app.compositor.panels[0].id;

    app.compositor
        .handle_xr_input(crate::XrInput::ClosePanel { panel_id });
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );

    assert_eq!(app.client.toplevel_closes, 1);
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
                state,
            } = receiver.try_recv().unwrap()
            else {
                panic!("mapped window must publish its own GPU image");
            };
            assert_eq!(panel_id, second_id);
            assert_eq!(geometry.pose, second_pose);
            assert_eq!(geometry.logical_size, expected_size.into());
            assert_eq!(state, crate::bridge::PanelState::default());
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
