use crate::{Compositor, Display, Ray3, bridge};
use glam::Vec3;

#[test]
#[ignore = "requires Xwayland and XDG_RUNTIME_DIR"]
fn x11_app_waits_for_gpu_and_receives_input() {
    exercise_x11_app(None);
}

#[test]
#[ignore = "requires Xwayland, XDG_RUNTIME_DIR, and Vulkan DMA-BUF import support"]
fn gpu_shared_x11_app_preserves_pixels_and_input() {
    exercise_x11_app(Some(crate::gpu::test_support::Vulkan::new().unwrap()));
}

fn exercise_x11_app(vulkan: Option<crate::gpu::test_support::Vulkan>) {
    use smithay::reexports::x11rb::wrapper::ConnectionExt as _;
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
    use std::{
        cell::RefCell,
        rc::Rc,
        time::{Duration, Instant},
    };

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
            compositor.render_pending().unwrap();
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
    let (sender, receiver) = bridge::new_panel_channel();
    let mut compositor = Compositor::new(display.handle(), sender);
    let readiness = Rc::new(RefCell::new(Vec::new()));
    let callback_readiness = readiness.clone();
    compositor.ready_callback = Some(Box::new(move |display| {
        callback_readiness.borrow_mut().push(display);
        Ok(())
    }));
    if let Some(vulkan) = &vulkan {
        compositor.configure_gpu(&vulkan.render_node).unwrap();
    }
    let display_number = crate::x11::start(&display.handle(), event_loop.handle())
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
    assert_eq!(*readiness.borrow(), vec![Some(display_name.clone())]);
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
                        | EventMask::KEY_PRESS
                        | EventMask::KEY_RELEASE
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
        let crate::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("expected a captured X11 frame");
        };
        let shared = crate::gpu::SharedImage::import(
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

    compositor.dispatch_key(30, true, 13);
    compositor.dispatch_key(30, false, 14);
    compositor.flush_clients();
    let mut key_pressed = false;
    let mut key_released = false;
    until(&mut event_loop, &mut compositor, |_| {
        while let Some(event) = connection.poll_for_event().unwrap() {
            match event {
                Event::KeyPress(event) => {
                    assert_eq!(event.detail, 38);
                    key_pressed = true;
                }
                Event::KeyRelease(event) => {
                    assert_eq!(event.detail, 38);
                    key_released = true;
                }
                _ => {}
            }
        }
        key_pressed && key_released
    });

    let menu = connection.generate_id().unwrap();
    connection
        .create_window(
            screen.root_depth,
            menu,
            screen.root,
            85,
            15,
            40,
            20,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0x00ffff00)
                .event_mask(
                    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
                ),
        )
        .unwrap();
    connection
        .change_property32(
            smithay::reexports::x11rb::protocol::xproto::PropMode::REPLACE,
            menu,
            smithay::reexports::x11rb::protocol::xproto::AtomEnum::WM_TRANSIENT_FOR,
            smithay::reexports::x11rb::protocol::xproto::AtomEnum::WINDOW,
            &[window],
        )
        .unwrap();
    connection.map_window(menu).unwrap();
    connection.flush().unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.x11_popups.len() == 1 && state.panels[0].bounds.size == (125, 50).into()
    });
    assert_eq!(compositor.panels.len(), 1);
    assert_eq!(compositor.panels[0].pose, pose);
    if let Some(vulkan) = &vulkan {
        let crate::PanelUpdate::GpuFrame { dmabuf, .. } = receiver.try_recv().unwrap() else {
            panic!("X11 menu frame missing");
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
            &pixels[(20 * 125 + 90) * 4..(20 * 125 + 90) * 4 + 4],
            &[255, 255, 0, 255]
        );
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }
    assert!(compositor.dispatch_ray(
        Ray3 {
            origin: Vec3::new(0.45, 0.0, 0.0),
            direction: Vec3::NEG_Z
        },
        15
    ));
    compositor.dispatch_button(true, 16);
    compositor.dispatch_button(false, 17);
    compositor.flush_clients();
    let mut menu_clicked = false;
    until(&mut event_loop, &mut compositor, |_| {
        while let Some(event) = connection.poll_for_event().unwrap() {
            if let Event::ButtonPress(event) = event {
                assert_eq!(event.event, menu);
                assert_eq!((event.event_x, event.event_y), (10, 10));
                menu_clicked = true;
            }
        }
        menu_clicked
    });
    assert_eq!(
        connection.get_input_focus().unwrap().reply().unwrap().focus,
        window
    );
    connection.destroy_window(menu).unwrap();
    connection.flush().unwrap();
    until(&mut event_loop, &mut compositor, |state| {
        state.x11_popups.is_empty() && state.panels[0].bounds.size == (100, 50).into()
    });

    if vulkan.is_none() {
        connection.destroy_window(window).unwrap();
        connection.flush().unwrap();
        until(&mut event_loop, &mut compositor, |state| {
            state.panels.is_empty()
        });
        assert!(compositor.active_panel.is_none());
        assert!(
            matches!(receiver.try_recv().unwrap(), crate::PanelUpdate::Removed { panel_id: removed } if removed == panel_id)
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
        matches!(receiver.try_recv().unwrap(), crate::PanelUpdate::Removed { panel_id: removed } if removed == panel_id)
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
