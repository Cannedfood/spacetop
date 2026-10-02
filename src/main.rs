use std::{process::Command, sync::Arc, thread, time::Instant};

use anyhow::Context;
use glam::Vec3;
mod bridge;
mod gpu;
mod panel;
mod xr;

use bridge::{PanelUpdate, XrInput};
use panel::{PanelGeometry, PanelPose, Ray3};
use smithay::{
    backend::input::ButtonState,
    delegate_compositor, delegate_dmabuf, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    input::{Seat, SeatHandler, SeatState, keyboard::XkbConfig},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop, Interest, LoopSignal, Mode as PollMode, PostAction, generic::Generic,
        },
        wayland_server::{
            Client, Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
        },
    },
    utils::{Physical, Raw, SERIAL_COUNTER, Size},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, get_parent, with_surface_tree_downward,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        output::{OutputHandler, OutputManagerState},
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};

#[derive(Default)]
struct ClientState {
    compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

struct ToplevelPanel {
    surface: ToplevelSurface,
    geometry: Option<PanelGeometry>,
    id: u64,
}

struct Compositor {
    display_handle: DisplayHandle,
    compositor_state: CompositorState,
    shm_state: ShmState,
    xdg_shell_state: XdgShellState,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
    output: Output,
    panels: Vec<ToplevelPanel>,
    gpu_renderer: Option<gpu::GpuRenderer>,
    dmabuf_state: DmabufState,
    dmabuf_global: Option<DmabufGlobal>,
    next_panel_id: u64,
    frame_sender: calloop::channel::SyncSender<PanelUpdate>,
    active_panel: Option<u64>,
    fatal_error: Option<anyhow::Error>,
    started_at: Instant,
}

impl Compositor {
    fn finish_dispatch(&mut self, signal: &LoopSignal) {
        self.flush_clients();
        if self.fatal_error.is_some() {
            signal.stop();
        }
    }

    fn handle_xr_input(&mut self, command: XrInput) {
        let result = match command {
            XrInput::Ray { ray, time_ms } => {
                self.dispatch_ray(ray, time_ms);
                Ok(())
            }
            XrInput::Button { pressed, time_ms } => {
                self.dispatch_button(pressed, time_ms);
                Ok(())
            }
            XrInput::GpuDevice { render_node } => self.configure_gpu(&render_node),
            XrInput::FatalError { message } => Err(anyhow::anyhow!(message)),
        };
        if let Err(error) = result {
            self.fatal_error = Some(error);
        }
    }

    fn flush_clients(&mut self) {
        if let Err(error) = self.display_handle.flush_clients() {
            eprintln!("failed to flush Wayland events: {error}");
        }
    }

    fn new(
        display_handle: DisplayHandle,
        frame_sender: calloop::channel::SyncSender<PanelUpdate>,
    ) -> Self {
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let output_manager_state = OutputManagerState::new();

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "spacetop-seat");
        seat.add_pointer();
        seat.add_keyboard(XkbConfig::default(), 200, 25)
            .expect("initialize XKB keyboard");

        let output = Output::new(
            "SPACETOP-0".into(),
            PhysicalProperties {
                size: Size::<i32, Raw>::from((0, 0)),
                subpixel: Subpixel::Unknown,
                make: "spacetop".into(),
                model: "virtual".into(),
            },
        );
        let mode = Mode {
            size: Size::<i32, Physical>::from((1280, 720)),
            refresh: 60_000,
        };
        output.set_preferred(mode);
        output.change_current_state(Some(mode), None, None, None);
        output.create_global::<Self>(&display_handle);

        Self {
            display_handle,
            compositor_state,
            shm_state,
            xdg_shell_state,
            _output_manager_state: output_manager_state,
            seat_state,
            seat,
            output,
            panels: Vec::new(),
            gpu_renderer: None,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            next_panel_id: 1,
            frame_sender,
            active_panel: None,
            fatal_error: None,
            started_at: Instant::now(),
        }
    }

    fn update_panel_from_commit(&mut self, surface: &WlSurface) -> Option<usize> {
        let index = self
            .panels
            .iter()
            .position(|panel| panel.surface.wl_surface() == surface)?;
        let logical_size =
            smithay::backend::renderer::utils::with_renderer_surface_state(surface, |state| {
                state.surface_size()
            })
            .flatten();

        let count = self.panels.len();
        self.panels[index].geometry = logical_size.map(|logical_size| {
            let panel_index = index as f32;
            PanelGeometry {
                pose: PanelPose {
                    center: Vec3::new(
                        panel_index * 0.65 - (count.saturating_sub(1) as f32 * 0.325),
                        0.0,
                        -1.6,
                    ),
                    yaw: (panel_index - (count.saturating_sub(1) as f32 / 2.0)) * 0.12,
                    width_m: 1.0,
                },
                logical_size,
            }
        });
        Some(index)
    }

    fn configure_gpu(&mut self, render_node: &std::path::Path) -> anyhow::Result<()> {
        let renderer = gpu::GpuRenderer::new(render_node)
            .context("mandatory GPU compositor initialization failed")?;
        let feedback = renderer.feedback(render_node)?;
        self.gpu_renderer = Some(renderer);
        if self.dmabuf_global.is_none() {
            self.dmabuf_global = Some(
                self.dmabuf_state
                    .create_global_with_default_feedback::<Self>(&self.display_handle, &feedback),
            );
        }
        eprintln!("GPU panel compositing enabled on {}", render_node.display());
        self.refresh_panels()
    }

    fn refresh_panels(&mut self) -> anyhow::Result<()> {
        for index in 0..self.panels.len() {
            self.capture_panel(index)?;
        }
        Ok(())
    }

    fn complete_frame_callbacks(&self, surface: &WlSurface) {
        let time_ms = self.started_at.elapsed().as_millis() as u32;
        with_surface_tree_downward(
            surface,
            (),
            |_, _, &()| TraversalAction::DoChildren(()),
            |_, states, &()| {
                for callback in states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .frame_callbacks
                    .drain(..)
                {
                    callback.done(time_ms);
                }
            },
            |_, _, &()| true,
        );
    }

    fn capture_panel(&mut self, index: usize) -> anyhow::Result<()> {
        let surface = self.panels[index].surface.wl_surface().clone();
        let Some(logical_size) =
            smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |state| {
                state.surface_size()
            })
            .flatten()
        else {
            return Ok(());
        };

        let scale = (512.0 / logical_size.w as f64)
            .min(512.0 / logical_size.h as f64)
            .min(1.0);
        let size = Size::<i32, smithay::utils::Buffer>::from((
            (logical_size.w as f64 * scale).round().max(1.0) as i32,
            (logical_size.h as f64 * scale).round().max(1.0) as i32,
        ));
        let Some(renderer) = self.gpu_renderer.as_mut() else {
            return Ok(());
        };
        let dmabuf = renderer
            .capture(&surface, size, scale)
            .context("mandatory GPU panel capture failed")?;
        let panel_id = self.panels[index].id;
        let Some(pose) = self.panels[index].geometry.map(|geometry| geometry.pose) else {
            return Ok(());
        };
        if let Err(error) = self.frame_sender.try_send(PanelUpdate::GpuFrame {
            panel_id,
            dmabuf,
            pose,
        }) {
            eprintln!("XR panel frame dropped: {error}");
        }
        self.complete_frame_callbacks(&surface);
        Ok(())
    }

    fn set_panel_active(&mut self, panel_id: Option<u64>, serial: smithay::utils::Serial) {
        for panel in &self.panels {
            let should_activate = Some(panel.id) == panel_id;
            let currently_active = self.active_panel == Some(panel.id);
            if should_activate != currently_active {
                panel.surface.with_pending_state(|state| {
                    if should_activate {
                        state.states.set(
                            smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                        );
                    } else {
                        state.states.unset(
                            smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                        );
                    }
                });
                panel.surface.send_configure();
            }
        }
        self.active_panel = panel_id;
        if let (Some(panel_id), Some(keyboard)) = (panel_id, self.seat.get_keyboard()) {
            if let Some(panel) = self.panels.iter().find(|panel| panel.id == panel_id) {
                keyboard.set_focus(self, Some(panel.surface.wl_surface().clone()), serial);
            }
        } else if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, None, serial);
        }
    }

    fn dispatch_ray(&mut self, ray: Ray3, time_ms: u32) -> bool {
        // The Vulkan bridge currently draws one swapchain texture per panel
        // layer index 0. Keep hit testing on the same visible panel.
        let displayed_panel_id = self.panels.iter().map(|panel| panel.id).min();
        let hit = self
            .panels
            .iter()
            .filter(|panel| panel.surface.alive() && Some(panel.id) == displayed_panel_id)
            .filter_map(|panel| {
                let hit = panel.geometry?.intersect(ray)?;
                Some((panel.surface.wl_surface().clone(), hit))
            })
            .min_by(|a, b| a.1.distance_m.total_cmp(&b.1.distance_m));
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        let focus = hit
            .as_ref()
            .map(|(surface, _)| (surface.clone(), smithay::utils::Point::from((0.0, 0.0))));
        let location = hit
            .as_ref()
            .map(|(_, hit)| {
                smithay::utils::Point::from((hit.surface_px.x as f64, hit.surface_px.y as f64))
            })
            .unwrap_or_else(|| pointer.current_location());
        pointer.motion(
            self,
            focus,
            &smithay::input::pointer::MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: time_ms,
            },
        );
        pointer.frame(self);
        hit.is_some()
    }

    fn dispatch_button(&mut self, pressed: bool, time_ms: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.button(
            self,
            &smithay::input::pointer::ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: time_ms,
                button: 0x110,
                state: if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
            },
        );
        pointer.frame(self);
        if pressed {
            let focused = pointer.current_focus();
            let panel_id = focused.and_then(|surface| {
                self.panels
                    .iter()
                    .find(|panel| panel.surface.wl_surface() == &surface)
                    .map(|panel| panel.id)
            });
            self.set_panel_active(panel_id, SERIAL_COUNTER.next_serial());
        }
    }
}

impl CompositorHandler for Compositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("client state is installed on connection")
            .compositor
    }
    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if let Some(index) = self.update_panel_from_commit(&root)
            && let Err(error) = self.capture_panel(index)
        {
            self.fatal_error = Some(error);
        }
    }
}

impl BufferHandler for Compositor {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}
impl DmabufHandler for Compositor {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: ImportNotifier,
    ) {
        if self
            .gpu_renderer
            .as_mut()
            .is_some_and(|renderer| renderer.import_dmabuf(&dmabuf))
        {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}
impl ShmHandler for Compositor {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.output.enter(surface.wl_surface());
        let panel_id = self.next_panel_id;
        self.panels.push(ToplevelPanel {
            surface: surface.clone(),
            geometry: None,
            id: panel_id,
        });
        self.next_panel_id = self.next_panel_id.saturating_add(1);
        let activate = self.active_panel.is_none();
        if activate {
            self.set_panel_active(Some(panel_id), SERIAL_COUNTER.next_serial());
        } else {
            let _ = surface.send_configure();
        }
    }
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let _ = surface.send_configure();
    }
    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
    }
    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        _positioner: PositionerState,
        token: u32,
    ) {
        surface.send_repositioned(token);
    }
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let removed: Vec<u64> = self
            .panels
            .iter()
            .filter(|panel| panel.surface.wl_surface() == surface.wl_surface())
            .map(|panel| panel.id)
            .collect();
        self.panels
            .retain(|panel| panel.surface.wl_surface() != surface.wl_surface());
        for panel_id in removed {
            let _ = self
                .frame_sender
                .try_send(PanelUpdate::Removed { panel_id });
        }
    }
}

impl SeatHandler for Compositor {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}
impl OutputHandler for Compositor {}

delegate_compositor!(Compositor);
delegate_dmabuf!(Compositor);
delegate_shm!(Compositor);
delegate_xdg_shell!(Compositor);
delegate_seat!(Compositor);
delegate_output!(Compositor);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|argument| argument == "--xr-client") {
        if args.iter().any(|argument| argument.starts_with("--app=")) {
            return Err("--app cannot be used with --xr-client".into());
        }
        let (_frame_sender, frame_receiver) = bridge::panel_channel();
        let (input_sender, _input_receiver) = bridge::input_channel();
        return xr::run(frame_receiver, input_sender).map_err(Into::into);
    }
    let app = parse_app_argument(&args)?;
    let mut event_loop: EventLoop<Compositor> = EventLoop::try_new()?;
    let display: Display<Compositor> = Display::new()?;
    let display_handle = display.handle();
    let (frame_sender, frame_receiver) = bridge::panel_channel();
    let (input_sender, input_receiver) = bridge::input_channel();
    let xr_input_sender = input_sender.clone();
    let xr_error_sender = input_sender.clone();
    thread::Builder::new()
        .name("spacetop-openxr".into())
        .spawn(move || {
            if let Err(error) = xr::run(frame_receiver, xr_input_sender) {
                let _ = xr_error_sender.send(XrInput::FatalError {
                    message: format!("OpenXR client stopped: {error:#}"),
                });
            }
        })?;
    let mut compositor = Compositor::new(display_handle.clone(), frame_sender);
    let socket = ListeningSocketSource::new_auto()?;
    let wayland_display = socket.socket_name().to_os_string();
    eprintln!("Wayland display: {}", wayland_display.to_string_lossy());
    event_loop
        .handle()
        .insert_source(socket, move |stream, _, compositor| {
            compositor
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
                .expect("failed to insert Wayland client");
        })?;
    if let Some(app) = app {
        let child = Command::new(&app)
            .env("WAYLAND_DISPLAY", &wayland_display)
            .spawn()
            .map_err(|error| {
                format!(
                    "failed to start app `{app}` on {}`: {error}",
                    wayland_display.to_string_lossy()
                )
            })?;
        eprintln!("Started `{app}` as process {}", child.id());
    }
    event_loop
        .handle()
        .insert_source(input_receiver, |event, _, compositor| {
            if let calloop::channel::Event::Msg(command) = event {
                compositor.handle_xr_input(command);
            }
        })?;
    event_loop.handle().insert_source(
        Generic::new(display, Interest::READ, PollMode::Level),
        |_, display, compositor| {
            unsafe {
                display.get_mut().dispatch_clients(compositor)?;
                display.get_mut().flush_clients()?;
            }
            Ok(PostAction::Continue)
        },
    )?;
    let signal = event_loop.get_signal();
    event_loop.run(None, &mut compositor, |compositor| {
        compositor.finish_dispatch(&signal)
    })?;
    if let Some(error) = compositor.fatal_error {
        return Err(error.into());
    }
    Ok(())
}

fn parse_app_argument(args: &[String]) -> Result<Option<String>, String> {
    let mut app = None;
    for argument in args {
        if let Some(value) = argument.strip_prefix("--app=") {
            if value.is_empty() {
                return Err("--app requires an executable name, e.g. --app=totem".into());
            }
            if app.replace(value.to_owned()).is_some() {
                return Err("--app may only be specified once".into());
            }
        } else if argument != "--xr-client" {
            return Err(format!(
                "unknown argument `{argument}`; supported: --app=PROGRAM"
            ));
        }
    }
    Ok(app)
}

#[cfg(test)]
mod compositor_tests;

#[cfg(test)]
mod cli_tests {
    use super::parse_app_argument;

    #[test]
    fn parses_optional_app_name() {
        assert_eq!(
            parse_app_argument(&["--app=totem".into()]),
            Ok(Some("totem".into()))
        );
    }

    #[test]
    fn rejects_empty_and_duplicate_app_arguments() {
        assert!(parse_app_argument(&["--app=".into()]).is_err());
        assert!(parse_app_argument(&["--app=totem".into(), "--app=vlc".into()]).is_err());
    }
}
