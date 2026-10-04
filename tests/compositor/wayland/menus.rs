use super::super::client::pump;
use super::fixture::WaylandApp;
use crate::Ray3;
use glam::Vec3;
use std::{io::Write, os::fd::AsFd};
use wayland_client::protocol::wl_shm;
use wayland_protocols::xdg::shell::client::xdg_positioner;

#[test]
fn popup_click_invalidates_only_its_owner() {
    let mut app = WaylandApp::new(None);
    let second_surface = app
        .client
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&app.qh, ());
    let second_xdg =
        app.client
            .shell
            .as_ref()
            .unwrap()
            .get_xdg_surface(&second_surface, &app.qh, ());
    let _second_toplevel = second_xdg.get_toplevel(&app.qh, ());
    second_surface.commit();
    let menu_surface = app
        .client
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&app.qh, ());
    let menu_xdg = app
        .client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&menu_surface, &app.qh, ());
    let positioner = app
        .client
        .shell
        .as_ref()
        .unwrap()
        .create_positioner(&app.qh, ());
    positioner.set_size(20, 10);
    positioner.set_anchor_rect(5, 5, 10, 10);
    let _menu = menu_xdg.get_popup(Some(&app.xdg_surface), &positioner, &app.qh, ());
    menu_surface.commit();
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert_eq!(app.compositor.panels.len(), 2);
    let owner_id = app.compositor.panels[0].id;
    let pose = app.compositor.panels[0].pose;
    app.compositor.dirty_panels.clear();
    assert!(app.compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: pose.center.normalize()
        },
        1
    ));
    app.compositor.dispatch_button(true, 2);
    app.compositor.dispatch_button(false, 3);
    assert_eq!(
        app.compositor.dirty_panels,
        std::collections::BTreeSet::from([owner_id])
    );
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
        xdg_surface,
        ..
    } = app;
    let window_geometry_origin = 5;
    xdg_surface.set_window_geometry(window_geometry_origin, window_geometry_origin, 90, 40);
    surface.commit();
    pump(display, compositor, queue, client, connection);
    compositor.dispatch_pointer_button(0x111, true, 16);
    compositor.dispatch_pointer_button(0x111, false, 17);
    let grab_serial = u32::from(compositor.input_serials.back().unwrap().0);
    let menu_surface = client.compositor.as_ref().unwrap().create_surface(qh, ());
    let menu_xdg = client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&menu_surface, qh, ());
    let positioner = client.shell.as_ref().unwrap().create_positioner(qh, ());
    positioner.set_size(40, 20);
    positioner.set_anchor_rect(80, 10, 1, 1);
    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
    let menu = menu_xdg.get_popup(Some(xdg_surface), &positioner, qh, ());
    menu.grab(client.seat.as_ref().unwrap(), grab_serial);
    menu_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(client.popup_configures.last(), Some(&(80, 10, 40, 20)));
    let mut menu_file = tempfile::tempfile().unwrap();
    menu_file
        .write_all(&[0, 255, 255, 255].repeat(40 * 20))
        .unwrap();
    let menu_pool =
        client
            .shm
            .as_ref()
            .unwrap()
            .create_pool(menu_file.as_fd(), 40 * 20 * 4, qh, ());
    let menu_buffer = menu_pool.create_buffer(0, 40, 20, 160, wl_shm::Format::Argb8888, qh, ());
    menu_surface.attach(Some(&menu_buffer), 0, 0);
    menu_surface.frame(qh, true);
    menu_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(
        compositor.panels.len(),
        1,
        "menus must not use another XR layer"
    );
    assert_eq!(compositor.panels[0].bounds.size, (120, 40).into());
    assert!(compositor.seat.get_keyboard().unwrap().is_grabbed());
    let panel = &compositor.panels[0];
    let geometry = panel.geometry.unwrap();
    let popup_location = client.popup_configures.last().unwrap();
    let target_x = (popup_location.0 + 10 + window_geometry_origin - panel.bounds.loc.x) as f32
        / geometry.logical_size.w as f32;
    let target_y = (popup_location.1 + 10 + window_geometry_origin - panel.bounds.loc.y) as f32
        / geometry.logical_size.h as f32;
    let local_offset = Vec3::new(
        (target_x - 0.5) * geometry.pose.width_m,
        (0.5 - target_y) * geometry.pose.width_m * geometry.logical_size.h as f32
            / geometry.logical_size.w as f32,
        0.0,
    );
    let target = geometry.pose.center + geometry.pose.orientation() * local_offset;
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: target.normalize()
        },
        18
    ));
    pump(display, compositor, queue, client, connection);
    assert_eq!(client.pointer_surface.as_ref(), Some(&menu_surface));
    let (pointer_x, pointer_y) = *client.motions.last().unwrap();
    assert!(
        (pointer_x - 10.0).abs() < 0.001 && (pointer_y - 10.0).abs() < 0.001,
        "popup pointer location was ({pointer_x}, {pointer_y})"
    );
    if let Some(vulkan) = vulkan.as_ref() {
        let crate::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("menu frame missing");
        };
        let shared = crate::gpu::SharedImage::import(
            &vulkan.instance,
            &vulkan.device,
            vulkan.physical_device,
            dmabuf,
        )
        .unwrap();
        let pixels = vulkan.readback(&shared, None).unwrap();
        assert_eq!(
            &pixels[(20 * 130 + 90) * 4..(20 * 130 + 90) * 4 + 4],
            &[255, 255, 0, 255]
        );
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }

    let child_surface = client.compositor.as_ref().unwrap().create_surface(qh, ());
    let child_xdg = client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&child_surface, qh, ());
    positioner.set_size(20, 10);
    positioner.set_anchor_rect(30, 5, 1, 1);
    let child = child_xdg.get_popup(Some(&menu_xdg), &positioner, qh, ());
    child.grab(client.seat.as_ref().unwrap(), grab_serial);
    child_surface.commit();
    pump(display, compositor, queue, client, connection);
    let child_buffer = menu_pool.create_buffer(0, 20, 10, 160, wl_shm::Format::Argb8888, qh, ());
    child_surface.attach(Some(&child_buffer), 0, 0);
    child_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(compositor.panels[0].bounds.size, (130, 40).into());
    let panel = &compositor.panels[0];
    let geometry = panel.geometry.unwrap();
    let menu_location = client.popup_configures[0];
    let child_location = client.popup_configures.last().unwrap();
    let target_x = (window_geometry_origin + menu_location.0 + child_location.0 + 10
        - panel.bounds.loc.x) as f32
        / geometry.logical_size.w as f32;
    let target_y = (window_geometry_origin + menu_location.1 + child_location.1 + 5
        - panel.bounds.loc.y) as f32
        / geometry.logical_size.h as f32;
    let local_offset = Vec3::new(
        (target_x - 0.5) * geometry.pose.width_m,
        (0.5 - target_y) * geometry.pose.width_m * geometry.logical_size.h as f32
            / geometry.logical_size.w as f32,
        0.0,
    );
    let target = geometry.pose.center + geometry.pose.orientation() * local_offset;
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: target.normalize()
        },
        19
    ));
    pump(display, compositor, queue, client, connection);
    assert_eq!(client.pointer_surface.as_ref(), Some(&child_surface));
    child.destroy();
    child_xdg.destroy();
    child_surface.destroy();
    pump(display, compositor, queue, client, connection);
    positioner.set_size(40, 20);
    positioner.set_anchor_rect(-20, -10, 1, 1);
    menu.reposition(&positioner, 123);
    pump(display, compositor, queue, client, connection);
    menu_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(client.repositioned, vec![123]);
    assert_eq!(compositor.panels[0].bounds.loc, (-15, -5).into());
    menu_surface.attach(None, 0, 0);
    menu_surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(
        compositor.panels[0].bounds,
        smithay::utils::Rectangle::new((5, 5).into(), (90, 40).into())
    );
    assert!(!compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::new(4.0, 0.0, 0.0),
            direction: Vec3::NEG_Z
        },
        20
    ));
    compositor.dispatch_button(true, 21);
    compositor.dispatch_button(false, 22);
    pump(display, compositor, queue, client, connection);
    assert!(client.popup_done >= 1);
    assert_eq!(compositor.panels[0].bounds.size, (90, 40).into());
    menu.destroy();
    menu_xdg.destroy();
    menu_surface.destroy();
    positioner.destroy();
    xdg_surface.set_window_geometry(0, 0, 100, 50);
    surface.commit();
    pump(display, compositor, queue, client, connection);
    compositor.set_panel_active(
        Some(compositor.panels[0].id),
        smithay::utils::SERIAL_COUNTER.next_serial(),
    );

    let rejected_surface = client.compositor.as_ref().unwrap().create_surface(qh, ());
    let rejected_xdg = client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&rejected_surface, qh, ());
    let rejected_positioner = client.shell.as_ref().unwrap().create_positioner(qh, ());
    rejected_positioner.set_size(20, 10);
    rejected_positioner.set_anchor_rect(0, 0, 1, 1);
    let rejected = rejected_xdg.get_popup(Some(xdg_surface), &rejected_positioner, qh, ());
    let done_before = client.popup_done;
    rejected.grab(client.seat.as_ref().unwrap(), 0);
    pump(display, compositor, queue, client, connection);
    assert_eq!(client.popup_done, done_before + 1);
    rejected.destroy();
    rejected_xdg.destroy();
    rejected_surface.destroy();
    rejected_positioner.destroy();
}
