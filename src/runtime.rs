use std::{ffi::OsString, sync::Arc, thread};

use smithay::{
    reexports::{
        calloop::{
            EventLoop, Interest, Mode as PollMode, PostAction,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};

use crate::{ClientState, Compositor, XrInput, bridge, config::AppConfig, input, x11, xr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayNames {
    pub wayland: OsString,
    pub x11: Option<String>,
}

pub(crate) type ReadyCallback = Box<dyn FnOnce(Option<String>) -> anyhow::Result<()>>;

pub fn run(
    on_ready: impl FnOnce(DisplayNames) -> anyhow::Result<()> + 'static,
    on_launcher_toggle: impl FnMut() -> anyhow::Result<()> + 'static,
) -> anyhow::Result<()> {
    let config = AppConfig::load()?;
    run_with_config(config, on_ready, on_launcher_toggle)
}

pub fn run_with_config(
    config: AppConfig,
    on_ready: impl FnOnce(DisplayNames) -> anyhow::Result<()> + 'static,
    mut on_launcher_toggle: impl FnMut() -> anyhow::Result<()> + 'static,
) -> anyhow::Result<()> {
    let mut event_loop: EventLoop<Compositor> = EventLoop::try_new()?;
    let display: Display<Compositor> = Display::new()?;
    let display_handle = display.handle();
    let (frame_sender, frame_receiver) = bridge::new_panel_channel();
    let (input_sender, input_receiver) = bridge::new_input_channel();
    let xr_input_sender = input_sender.clone();
    let xr_error_sender = input_sender.clone();
    let xr_config = config.clone();
    thread::Builder::new()
        .name("spacetop-openxr".into())
        .spawn(move || {
            if let Err(error) = xr::run(frame_receiver, xr_input_sender, xr_config) {
                let _ = xr_error_sender.send(XrInput::FatalError {
                    message: format!("OpenXR client stopped: {error:#}"),
                });
            }
        })?;
    let mut compositor =
        Compositor::with_window_settings(display_handle.clone(), frame_sender, &config.window);
    input::start(&event_loop.handle())?;
    let socket = ListeningSocketSource::new_auto()?;
    let wayland_display = socket.socket_name().to_os_string();
    eprintln!("Wayland display: {}", wayland_display.to_string_lossy());
    compositor.ready_callback = Some(Box::new(move |x11| {
        on_ready(DisplayNames {
            wayland: wayland_display,
            x11,
        })
    }));
    let waiting_for_xwayland = x11::start(&display_handle, event_loop.handle())?;
    event_loop
        .handle()
        .insert_source(socket, move |stream, _, compositor| {
            compositor
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
                .expect("failed to insert Wayland client");
        })?;
    if waiting_for_xwayland.is_none() {
        compositor.notify_ready(None)?;
    }
    event_loop
        .handle()
        .insert_source(input_receiver, move |event, _, compositor| {
            if let calloop::channel::Event::Msg(command) = event {
                input_sender.received(&command);
                let started = std::time::Instant::now();
                match command {
                    XrInput::LauncherToggle => {
                        if let Err(error) = on_launcher_toggle() {
                            eprintln!("Could not toggle launcher: {error:#}");
                        }
                    }
                    other => compositor.handle_xr_input(other),
                }
                compositor.timings.record(
                    "app/input-dispatch",
                    started.elapsed(),
                    std::time::Duration::ZERO,
                );
            }
        })
        .map_err(|error| anyhow::anyhow!("failed to register XR input: {error}"))?;
    event_loop.handle().insert_source(
        Generic::new(display, Interest::READ, PollMode::Level),
        |_, display, compositor| {
            let started = std::time::Instant::now();
            unsafe {
                display.get_mut().dispatch_clients(compositor)?;
            }
            compositor.timings.record(
                "app/wayland-dispatch",
                started.elapsed(),
                std::time::Duration::ZERO,
            );
            let started = std::time::Instant::now();
            unsafe {
                display.get_mut().flush_clients()?;
            }
            compositor.timings.record(
                "app/wayland-flush",
                started.elapsed(),
                std::time::Duration::ZERO,
            );
            Ok(PostAction::Continue)
        },
    )?;
    event_loop
        .handle()
        .insert_source(
            Timer::from_duration(std::time::Duration::from_millis(100)),
            |_, _, compositor| {
                compositor.hide_idle_mouse_cursor();
                TimeoutAction::ToDuration(std::time::Duration::from_millis(100))
            },
        )
        .map_err(|error| anyhow::anyhow!("register mouse cursor idle timer: {error}"))?;
    let signal = event_loop.get_signal();
    event_loop.run(None, &mut compositor, |compositor| {
        compositor.finish_dispatch(&signal)
    })?;
    if let Some(error) = compositor.fatal_error {
        return Err(error);
    }
    Ok(())
}

pub fn run_xr_client() -> anyhow::Result<()> {
    let config = AppConfig::load()?;
    let (_frame_sender, frame_receiver) = bridge::new_panel_channel();
    xr::run(frame_receiver, bridge::InputSender::discarded(), config)
}
