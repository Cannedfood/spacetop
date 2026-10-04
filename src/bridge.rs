//! Latest panel updates, reliable transitions, and capped best-effort XR input.

use calloop::channel::{self, Channel, Sender};
use smithay::backend::allocator::dmabuf::Dmabuf;
#[cfg(test)]
use std::sync::mpsc::TryRecvError;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::panel::{PanelGeometry, PanelLimits, PanelPose, Ray3};

#[derive(Debug)]
pub enum XrInput {
    FrameTick,
    Ray {
        ray: Ray3,
        gaze_ray: Option<Ray3>,
        time_ms: u32,
    },
    GazeRay {
        ray: Ray3,
    },
    Button {
        button: u32,
        pressed: bool,
        time_ms: u32,
    },
    PointerLost {
        time_ms: u32,
    },
    Scroll {
        value: f64,
        time_ms: u32,
    },
    MovePanel {
        panel_id: u64,
        pose: PanelPose,
    },
    ResizePanel {
        panel_id: u64,
        width: i32,
        height: i32,
        anchor: Option<(PanelGeometry, [bool; 4])>,
    },
    GpuDevice {
        render_node: std::path::PathBuf,
        limits: PanelLimits,
    },
    ConfigReloaded {
        default_window_distance: f32,
        window_pixels_per_degree: f32,
    },
    FatalError {
        message: String,
    },
}

#[derive(Clone)]
pub struct InputSender(Option<Arc<InputQueue>>);

struct InputQueue {
    sender: Sender<XrInput>,
    pending: AtomicUsize,
    frame_pending: AtomicBool,
}

impl InputSender {
    pub fn discarded() -> Self {
        Self(None)
    }

    pub fn send(&self, input: XrInput) -> Result<(), std::sync::mpsc::SendError<XrInput>> {
        if let Some(queue) = &self.0 {
            queue.pending.fetch_add(1, Ordering::Relaxed);
            queue.sender.send(input).inspect_err(|_| {
                queue.pending.fetch_sub(1, Ordering::Relaxed);
            })
        } else {
            Ok(())
        }
    }

    pub fn try_send(&self, input: XrInput) -> Result<(), std::sync::mpsc::TrySendError<XrInput>> {
        if let Some(queue) = &self.0 {
            if queue
                .pending
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |pending| {
                    (pending < 16).then_some(pending + 1)
                })
                .is_err()
            {
                return Err(std::sync::mpsc::TrySendError::Full(input));
            }
            queue.sender.send(input).map_err(|error| {
                queue.pending.fetch_sub(1, Ordering::Relaxed);
                std::sync::mpsc::TrySendError::Disconnected(error.0)
            })
        } else {
            Ok(())
        }
    }

    pub fn request_frame(&self) -> Result<(), std::sync::mpsc::SendError<XrInput>> {
        if let Some(queue) = &self.0 {
            if queue.frame_pending.swap(true, Ordering::Relaxed) {
                return Ok(());
            }
            self.send(XrInput::FrameTick).inspect_err(|_| {
                queue.frame_pending.store(false, Ordering::Relaxed);
            })
        } else {
            Ok(())
        }
    }

    pub fn received(&self, input: &XrInput) {
        if let Some(queue) = &self.0 {
            queue.pending.fetch_sub(1, Ordering::Relaxed);
            if matches!(input, XrInput::FrameTick) {
                queue.frame_pending.store(false, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Debug)]
pub enum PanelUpdate {
    GpuFrame {
        panel_id: u64,
        dmabuf: Dmabuf,
        geometry: PanelGeometry,
    },
    Removed {
        panel_id: u64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct CursorState {
    pub mouse_controlled: bool,
    pub pose: Option<PanelPose>,
}

#[derive(Clone)]
pub struct PanelSender {
    updates: Arc<Mutex<BTreeMap<u64, PanelUpdate>>>,
    cursor: Arc<Mutex<Option<CursorState>>>,
}

pub struct PanelReceiver {
    updates: Arc<Mutex<BTreeMap<u64, PanelUpdate>>>,
    cursor: Arc<Mutex<Option<CursorState>>>,
}

impl PanelSender {
    pub fn publish(&self, update: PanelUpdate) {
        let panel_id = match &update {
            PanelUpdate::GpuFrame { panel_id, .. } | PanelUpdate::Removed { panel_id } => *panel_id,
        };
        self.updates
            .lock()
            .expect("panel updates lock")
            .insert(panel_id, update);
    }

    pub fn publish_cursor(&self, cursor: CursorState) {
        *self.cursor.lock().expect("cursor state lock") = Some(cursor);
    }
}

impl PanelReceiver {
    pub fn drain(&self) -> Vec<PanelUpdate> {
        std::mem::take(&mut *self.updates.lock().expect("panel updates lock"))
            .into_values()
            .collect()
    }

    pub fn take_cursor(&self) -> Option<CursorState> {
        self.cursor.lock().expect("cursor state lock").take()
    }

    #[cfg(test)]
    pub fn try_recv(&self) -> Result<PanelUpdate, TryRecvError> {
        self.updates
            .lock()
            .expect("panel updates lock")
            .pop_first()
            .map(|(_, update)| update)
            .ok_or(TryRecvError::Empty)
    }
}

pub fn panel_channel() -> (PanelSender, PanelReceiver) {
    let updates = Arc::new(Mutex::new(BTreeMap::new()));
    let cursor = Arc::new(Mutex::new(None));
    (
        PanelSender {
            updates: updates.clone(),
            cursor: cursor.clone(),
        },
        PanelReceiver { updates, cursor },
    )
}

pub fn input_channel() -> (InputSender, Channel<XrInput>) {
    let (sender, receiver) = channel::channel();
    (
        InputSender(Some(Arc::new(InputQueue {
            sender,
            pending: AtomicUsize::new(0),
            frame_pending: AtomicBool::new(false),
        }))),
        receiver,
    )
}

#[cfg(test)]
#[path = "../tests/unit/bridge.rs"]
mod tests;
