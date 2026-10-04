use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use anyhow::Context;
use evdev::{Device, EventType, RelativeAxisCode};
use smithay::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction, RegistrationToken,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};

use crate::Compositor;

pub fn transition(counts: &mut BTreeMap<u32, usize>, code: u32, pressed: bool) -> bool {
    if pressed {
        let count = counts.entry(code).or_default();
        *count += 1;
        *count == 1
    } else if let Some(count) = counts.get_mut(&code) {
        *count -= 1;
        if *count == 0 {
            counts.remove(&code);
            true
        } else {
            false
        }
    } else {
        false
    }
}

const RESCAN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
enum Selection {
    Automatic { seat: OsString },
    Explicit(Vec<PathBuf>),
}

impl Selection {
    fn new(paths: Option<OsString>, seat: Option<OsString>) -> Self {
        match paths {
            Some(paths) if paths.is_empty() => Self::Explicit(Vec::new()),
            Some(paths) => Self::Explicit(std::env::split_paths(&paths).collect()),
            None => Self::Automatic {
                seat: seat
                    .filter(|seat| !seat.is_empty())
                    .unwrap_or_else(|| "seat0".into()),
            },
        }
    }

    fn paths(&self) -> anyhow::Result<BTreeSet<PathBuf>> {
        match self {
            Self::Explicit(paths) => Ok(paths.iter().cloned().collect()),
            Self::Automatic { seat } => {
                let mut enumerator = udev::Enumerator::new().context("create input enumerator")?;
                enumerator.match_subsystem("input")?;
                enumerator.match_sysname("event*")?;
                enumerator.match_is_initialized()?;
                Ok(enumerator
                    .scan_devices()
                    .context("enumerate input devices")?
                    .filter(|device| {
                        eligible_device(
                            seat,
                            device.property_value("ID_SEAT"),
                            device.property_value("ID_INPUT_KEYBOARD"),
                            device.property_value("ID_INPUT_MOUSE"),
                        )
                    })
                    .filter_map(|device| device.devnode().map(Path::to_owned))
                    .collect())
            }
        }
    }
}

fn eligible_device(
    seat: &OsStr,
    device_seat: Option<&OsStr>,
    keyboard: Option<&OsStr>,
    mouse: Option<&OsStr>,
) -> bool {
    device_seat.unwrap_or_else(|| OsStr::new("seat0")) == seat
        && (keyboard == Some(OsStr::new("1")) || mouse == Some(OsStr::new("1")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DeviceIdentity {
    dev: u64,
    ino: u64,
    rdev: u64,
}

impl DeviceIdentity {
    fn at(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            rdev: metadata.rdev(),
        })
    }
}

#[derive(Default)]
struct DeviceState {
    pressed: BTreeSet<u16>,
    disconnected: bool,
}

impl DeviceState {
    fn release_all(&mut self, mut dispatch: impl FnMut(u16)) {
        for code in std::mem::take(&mut self.pressed) {
            dispatch(code);
        }
    }
}

struct TrackedDevice {
    identity: DeviceIdentity,
    token: RegistrationToken,
    state: Rc<RefCell<DeviceState>>,
}

impl TrackedDevice {
    fn needs_removal(&self, identity: Option<&DeviceIdentity>) -> bool {
        self.state.borrow().disconnected || identity != Some(&self.identity)
    }
}

struct DeviceManager {
    selection: Selection,
    devices: BTreeMap<PathBuf, TrackedDevice>,
    failures: BTreeMap<PathBuf, String>,
}

impl DeviceManager {
    fn report_failure(&mut self, path: &Path, error: &anyhow::Error) {
        let message = format!("{error:#}");
        if self.failures.get(path) != Some(&message) {
            eprintln!(
                "Input device {} unavailable: {message}; will retry",
                path.display()
            );
            self.failures.insert(path.to_owned(), message);
        }
    }

    fn identities(&mut self, paths: &BTreeSet<PathBuf>) -> BTreeMap<PathBuf, DeviceIdentity> {
        self.failures.retain(|path, _| paths.contains(path));
        let mut identities = BTreeMap::new();
        for path in paths {
            match DeviceIdentity::at(path) {
                Ok(identity) => {
                    identities.insert(path.clone(), identity);
                }
                Err(error) => self.report_failure(path, &error.into()),
            }
        }
        identities
    }

    fn connect(
        &mut self,
        handle: &LoopHandle<'static, Compositor>,
        identities: BTreeMap<PathBuf, DeviceIdentity>,
        strict: bool,
    ) -> anyhow::Result<()> {
        for (path, identity) in identities {
            if self.devices.contains_key(&path)
                || self
                    .devices
                    .values()
                    .any(|device| device.identity == identity)
            {
                continue;
            }
            match add_device(handle, &path, identity) {
                Ok(device) => {
                    eprintln!("Listening to input device {}", path.display());
                    self.failures.remove(&path);
                    self.devices.insert(path, device);
                }
                Err(error) if strict => return Err(error),
                Err(error) => self.report_failure(&path, &error),
            }
        }
        Ok(())
    }

    fn refresh(
        &mut self,
        handle: &LoopHandle<'static, Compositor>,
        compositor: &mut Compositor,
    ) -> anyhow::Result<()> {
        let paths = self.selection.paths()?;
        let identities = self.identities(&paths);
        self.devices.retain(|path, device| {
            if !device.needs_removal(identities.get(path)) {
                return true;
            }
            let mut state = device.state.borrow_mut();
            if !state.disconnected {
                handle.remove(device.token);
                eprintln!("Input device {} removed", path.display());
            }
            let time_ms = compositor.started_at.elapsed().as_millis() as u32;
            state.release_all(|code| dispatch_code(compositor, code, false, time_ms));
            false
        });
        self.connect(handle, identities, false)
    }
}

pub fn start(handle: &LoopHandle<'static, Compositor>) -> anyhow::Result<()> {
    let selection = Selection::new(
        std::env::var_os("SPACETOP_INPUT_DEVICES"),
        std::env::var_os("XDG_SEAT"),
    );
    let strict = matches!(selection, Selection::Explicit(_));
    let paths = selection.paths()?;
    let mut manager = DeviceManager {
        selection,
        devices: BTreeMap::new(),
        failures: BTreeMap::new(),
    };
    let identities = if strict {
        paths
            .iter()
            .map(|path| {
                DeviceIdentity::at(path)
                    .with_context(|| format!("inspect configured input device {}", path.display()))
                    .map(|identity| (path.clone(), identity))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?
    } else {
        manager.identities(&paths)
    };
    manager.connect(handle, identities, strict)?;
    if manager.devices.is_empty() && !strict {
        eprintln!("No readable keyboards/mice found on this seat; watching for input devices");
    }
    let handle_for_timer = handle.clone();
    handle
        .insert_source(
            Timer::from_duration(RESCAN_INTERVAL),
            move |_, _, compositor| {
                if let Err(error) = manager.refresh(&handle_for_timer, compositor) {
                    eprintln!("Input discovery failed: {error:#}; will retry");
                }
                TimeoutAction::ToDuration(RESCAN_INTERVAL)
            },
        )
        .map_err(|error| anyhow::anyhow!("register input hotplug timer: {error}"))?;
    Ok(())
}

fn dispatch_code(compositor: &mut Compositor, code: u16, down: bool, time_ms: u32) {
    if (0x110..=0x117).contains(&code) {
        compositor.dispatch_pointer_button(code as u32, down, time_ms);
    } else if !(0x100..=0x15f).contains(&code) {
        compositor.dispatch_key(code as u32, down, time_ms);
    }
}

fn add_device(
    handle: &LoopHandle<'static, Compositor>,
    path: &Path,
    identity: DeviceIdentity,
) -> anyhow::Result<TrackedDevice> {
    let device = Device::open(path).with_context(|| {
        format!(
            "open input device {}; grant your user read access to this device",
            path.display()
        )
    })?;
    device.set_nonblocking(true)?;
    anyhow::ensure!(
        device.supported_keys().is_some(),
        "{} has no keys/buttons",
        path.display()
    );
    let state = Rc::new(RefCell::new(DeviceState::default()));
    let callback_state = state.clone();
    let path = path.to_owned();
    let token = handle
        .insert_source(
            Generic::new(device, Interest::READ, Mode::Level),
            move |_, device, compositor| {
                let mut state = callback_state.borrow_mut();
                let time_ms = compositor.started_at.elapsed().as_millis() as u32;
                let events = unsafe { device.get_mut() }
                    .fetch_events()
                    .map(|events| events.collect::<Vec<_>>());
                match events {
                    Ok(events) => {
                        for event in events {
                            if event.event_type() == EventType::KEY && event.value() != 2 {
                                let code = event.code();
                                let down = event.value() != 0;
                                let changed = if down {
                                    state.pressed.insert(code)
                                } else {
                                    state.pressed.remove(&code)
                                };
                                if changed {
                                    dispatch_code(compositor, code, down, time_ms);
                                }
                            } else if event.event_type() == EventType::RELATIVE {
                                if event.code() == RelativeAxisCode::REL_WHEEL.0 {
                                    compositor.dispatch_wheel(false, event.value(), time_ms);
                                } else if event.code() == RelativeAxisCode::REL_HWHEEL.0 {
                                    compositor.dispatch_wheel(true, event.value(), time_ms);
                                }
                            }
                        }
                        Ok(PostAction::Continue)
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        Ok(PostAction::Continue)
                    }
                    Err(error) => {
                        eprintln!("Input device {} disconnected: {error}", path.display());
                        state.disconnected = true;
                        state.release_all(|code| dispatch_code(compositor, code, false, time_ms));
                        Ok(PostAction::Remove)
                    }
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("register input device: {error}"))?;
    Ok(TrackedDevice {
        identity,
        token,
        state,
    })
}

#[cfg(test)]
#[path = "../tests/unit/input.rs"]
mod tests;
