use crate::{Compositor, Display, bridge};

#[test]
fn readiness_callback_runs_once_with_or_without_xwayland() {
    use std::{cell::RefCell, rc::Rc};

    for display_name in [None, Some(":test".to_owned())] {
        let display = Display::<Compositor>::new().unwrap();
        let (sender, _receiver) = bridge::panel_channel();
        let mut compositor = Compositor::new(display.handle(), sender);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let callback_calls = calls.clone();
        compositor.ready_callback = Some(Box::new(move |display| {
            callback_calls.borrow_mut().push(display);
            Ok(())
        }));
        compositor.notify_ready(display_name.clone()).unwrap();
        compositor.notify_ready(display_name.clone()).unwrap();
        assert_eq!(*calls.borrow(), vec![display_name]);
    }
}

#[test]
fn readiness_callback_errors_are_returned() {
    let display = Display::<Compositor>::new().unwrap();
    let (sender, _receiver) = bridge::panel_channel();
    let mut compositor = Compositor::new(display.handle(), sender);
    compositor.ready_callback = Some(Box::new(|_| anyhow::bail!("application startup failed")));
    assert_eq!(
        compositor.notify_ready(None).unwrap_err().to_string(),
        "application startup failed"
    );
    assert!(compositor.ready_callback.is_none());
}

#[test]
fn config_reload_updates_default_window_distance() {
    let display = Display::<Compositor>::new().unwrap();
    let (sender, _receiver) = bridge::panel_channel();
    let mut compositor = Compositor::new(display.handle(), sender);

    compositor.handle_xr_input(crate::XrInput::ConfigReloaded {
        default_window_distance: 2.4,
        window_pixels_per_degree: 32.0,
    });

    assert_eq!(compositor.default_window_distance, 2.4);
    assert!(compositor.fatal_error.is_none());
}

#[test]
fn gpu_and_xr_failures_are_fatal() {
    let directory = tempfile::tempdir().unwrap();
    for command in [
        crate::XrInput::GpuDevice {
            limits: crate::panel::PanelLimits::default(),
            render_node: directory.path().join("missing-render-node"),
        },
        crate::XrInput::FatalError {
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
