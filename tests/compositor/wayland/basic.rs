use super::super::client::pump;
use super::fixture::WaylandApp;
use crate::Ray3;
use glam::Vec3;
use wayland_client::protocol::wl_pointer;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1;

#[test]
fn clipboard_transfers_data_and_follows_keyboard_focus() {
    use std::{
        io::Read,
        os::{fd::AsFd, unix::net::UnixStream},
        time::Duration,
    };

    let mut app = WaylandApp::new(None);
    let source = app
        .client
        .data_manager
        .as_ref()
        .unwrap()
        .create_data_source(&app.qh, ());
    source.offer("text/plain;charset=utf-8".into());
    app.client.selection_data = b"spacetop clipboard".to_vec();
    let pointer_pose = app.compositor.panels[0].pose;
    app.compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::ZERO,
            direction: pointer_pose.center.normalize(),
        },
        1,
    );
    app.compositor.dispatch_button(true, 2);
    app.compositor.dispatch_button(false, 3);
    let serial = app.compositor.input_serials.back().unwrap().0;
    app.client
        .data_device
        .as_ref()
        .unwrap()
        .set_selection(Some(&source), serial.into());
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert!(
        app.client
            .selection_mime_types
            .iter()
            .any(|mime| mime == "text/plain;charset=utf-8")
    );
    let offer = app.client.selection_offer.as_ref().unwrap().clone();
    let (mut reader, writer) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    offer.receive("text/plain;charset=utf-8".into(), writer.as_fd());
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    let mut received = vec![0; app.client.selection_data.len()];
    reader.read_exact(&mut received).unwrap();
    assert_eq!(received, app.client.selection_data);
    app.compositor
        .set_panel_active(None, smithay::utils::SERIAL_COUNTER.next_serial());
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert!(
        app.compositor
            .seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .is_none()
    );
    app.client.selection_offer = None;
    app.client.selection_mime_types.clear();
    let panel_id = app.compositor.panels[0].id;
    app.compositor
        .set_panel_active(Some(panel_id), smithay::utils::SERIAL_COUNTER.next_serial());
    pump(
        &mut app.display,
        &mut app.compositor,
        &mut app.queue,
        &mut app.client,
        &app.connection,
    );
    assert!(app.client.selection_offer.is_some());
    assert_eq!(
        app.client.selection_mime_types,
        ["text/plain;charset=utf-8"]
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
        ..
    } = app;
    assert_eq!(
        client.frames, 0,
        "redraws must wait until GPU setup completes"
    );
    assert!(receiver.try_recv().is_err());
    if let Some(vulkan) = vulkan.as_ref() {
        compositor.configure_gpu(&vulkan.render_node).unwrap();
        pump(display, compositor, queue, client, connection);
        assert_eq!(client.frames, 1, "GPU setup must resume pending redraws");
    }
    assert!(
        client.keyboard_entered,
        "the initial active app must receive keyboard focus"
    );
    if let Some(vulkan) = vulkan.as_ref() {
        let crate::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("GPU capture must publish a DMA-BUF, not a CPU snapshot");
        };
        let shared = crate::gpu::SharedImage::import(
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
        let quad_pixels = vulkan.readback_cursor().unwrap();
        assert_eq!(quad_pixels.len(), 21 * 21 * 4);
        for (index, pixel) in quad_pixels.as_chunks::<4>().0.iter().enumerate() {
            let expected = if index % 21 == 10 || index / 21 == 10 {
                [255, 245, 0, 255]
            } else {
                [0, 0, 0, 0]
            };
            assert_eq!(*pixel, expected, "incorrect cursor quad pixel {index}");
        }

        let params = client
            .dmabuf
            .as_ref()
            .expect("GPU compositor must advertise linux-dmabuf")
            .create_params(qh, ());
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
            qh,
            (),
        );
        surface.attach(Some(&gpu_buffer), 0, 0);
        surface.damage(0, 0, 100, 50);
        surface.frame(qh, true);
        surface.commit();
        pump(display, compositor, queue, client, connection);
        assert_eq!(client.frames, 2);
        let crate::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("GPU-backed app buffers must remain on the shared path");
        };
        let imported = crate::gpu::SharedImage::import(
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

    compositor.dispatch_scroll(-12.5, 9);
    compositor.flush_clients();
    assert!(client.axis_values.is_empty());
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
            queue.blocking_dispatch(client).unwrap();
        }
    }
    assert!(client.entered);
    assert_eq!(client.motions.last(), Some(&(60.0, 25.0)));
    compositor.dispatch_scroll(-12.5, 9);
    compositor.flush_clients();
    while client.pointer_frames < 3 {
        queue.blocking_dispatch(client).unwrap();
    }
    assert_eq!(client.axis_values, vec![-12.5]);
    let pointer_frames_before_buttons = client.pointer_frames;
    for (step, pressed) in [true, false].into_iter().enumerate() {
        compositor.dispatch_button(pressed, 10 + step as u32);
        compositor.flush_clients();
        while client.pointer_frames < pointer_frames_before_buttons + step + 1 {
            queue.blocking_dispatch(client).unwrap();
        }
    }
    assert_eq!(
        client.buttons,
        vec![
            wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
            wayland_client::WEnum::Value(wl_pointer::ButtonState::Released),
        ]
    );
    surface.frame(qh, true);
    surface.commit();
    pump(display, compositor, queue, client, connection);
    assert_eq!(
        client.frames,
        if vulkan.is_some() { 3 } else { 0 },
        "subsequent redraws must also complete"
    );

    for (code, pressed) in [(42, true), (30, true), (30, false), (42, false)] {
        compositor.dispatch_key(code, pressed, 15);
    }
    pump(display, compositor, queue, client, connection);
    assert_eq!(
        client
            .keys
            .iter()
            .map(|(code, _)| *code)
            .collect::<Vec<_>>(),
        vec![42, 30, 30, 42]
    );
    assert!(client.modifier_masks.iter().any(|mask| *mask != 0));
    assert_eq!(client.modifier_masks.last(), Some(&0));
}
