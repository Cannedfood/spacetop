use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use anyhow::Context;
use evdev::{Device, EventType, RelativeAxisCode};
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic};

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

pub fn start(handle: &LoopHandle<'static, Compositor>) -> anyhow::Result<()> {
    let Some(paths) = std::env::var_os("SPACETOP_INPUT_DEVICES") else {
        eprintln!(
            "No physical input devices configured; set SPACETOP_INPUT_DEVICES to keyboard/mouse event paths"
        );
        return Ok(());
    };
    for path in std::env::split_paths(&paths) {
        add_device(handle, &path)?;
    }
    Ok(())
}

fn add_device(handle: &LoopHandle<'static, Compositor>, path: &Path) -> anyhow::Result<()> {
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
    let mut pressed = BTreeSet::new();
    let path = path.to_owned();
    handle
        .insert_source(
            Generic::new(device, Interest::READ, Mode::Level),
            move |_, device, compositor| {
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
                                    pressed.insert(code)
                                } else {
                                    pressed.remove(&code)
                                };
                                if changed {
                                    if (0x110..=0x117).contains(&code) {
                                        compositor.dispatch_pointer_button(
                                            code as u32,
                                            down,
                                            time_ms,
                                        );
                                    } else if !(0x100..=0x15f).contains(&code) {
                                        compositor.dispatch_key(code as u32, down, time_ms);
                                    }
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
                        for code in std::mem::take(&mut pressed) {
                            if (0x110..=0x117).contains(&code) {
                                compositor.dispatch_pointer_button(code as u32, false, time_ms);
                            } else if !(0x100..=0x15f).contains(&code) {
                                compositor.dispatch_key(code as u32, false, time_ms);
                            }
                        }
                        Ok(PostAction::Remove)
                    }
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("register input device: {error}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/input.rs"]
mod tests;
