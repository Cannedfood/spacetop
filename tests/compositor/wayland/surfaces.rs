use super::super::client::pump;
use super::fixture::WaylandApp;
use crate::Ray3;
use glam::Vec3;
use std::{io::Write, os::fd::AsFd};
use wayland_client::protocol::{wl_pointer, wl_shm};

pub(super) fn exercise(app: &mut WaylandApp) {
    let WaylandApp {
        display,
        compositor,
        connection,
        queue,
        qh,
        client,
        surface,
        ..
    } = app;
    let mut pixels = tempfile::tempfile().unwrap();
    pixels
        .write_all(&[0, 255, 255, 255].repeat(40 * 20))
        .unwrap();
    let menu_pool = client
        .shm
        .as_ref()
        .unwrap()
        .create_pool(pixels.as_fd(), 40 * 20 * 4, qh, ());
    let sub_surface = client.compositor.as_ref().unwrap().create_surface(qh, ());
    let sub = client
        .subcompositor
        .as_ref()
        .unwrap()
        .get_subsurface(&sub_surface, surface, qh, ());
    sub.set_position(20, 10);
    sub.set_desync();
    let sub_buffer = menu_pool.create_buffer(0, 20, 10, 160, wl_shm::Format::Argb8888, qh, ());
    sub_surface.attach(Some(&sub_buffer), 0, 0);
    sub_surface.commit();
    surface.commit();
    pump(display, compositor, queue, client, connection);
    let panel = &compositor.panels[0];
    let geometry = panel.geometry.unwrap();
    let target_x = (30 - panel.bounds.loc.x) as f32 / geometry.logical_size.w as f32;
    let target_y = (15 - panel.bounds.loc.y) as f32 / geometry.logical_size.h as f32;
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
        23
    ));
    pump(display, compositor, queue, client, connection);
    assert_eq!(client.pointer_surface.as_ref(), Some(&sub_surface));
    let (pointer_x, pointer_y) = *client.motions.last().unwrap();
    assert!((pointer_x - 10.0).abs() < 0.001 && (pointer_y - 5.0).abs() < 0.001);
    compositor.handle_xr_input(crate::XrInput::Button {
        button: 0x110,
        pressed: true,
        time_ms: 24,
    });
    compositor.handle_xr_input(crate::XrInput::PointerLost { time_ms: 25 });
    pump(display, compositor, queue, client, connection);
    assert_eq!(
        client.buttons.last(),
        Some(&wayland_client::WEnum::Value(
            wl_pointer::ButtonState::Released
        ))
    );
    assert!(
        compositor
            .seat
            .get_pointer()
            .unwrap()
            .current_focus()
            .is_none()
    );
    assert!(compositor.xr_buttons.is_empty());

    let region = client.compositor.as_ref().unwrap().create_region(qh, ());
    region.add(0, 0, 20, 50);
    surface.set_input_region(Some(&region));
    surface.commit();
    pump(display, compositor, queue, client, connection);
    assert!(!compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: geometry.pose.center.normalize()
        },
        26
    ));
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: target.normalize()
        },
        27
    ));
    sub.destroy();
    sub_surface.destroy();
    surface.set_input_region(None);
    region.destroy();
    surface.commit();
    pump(display, compositor, queue, client, connection);
}
