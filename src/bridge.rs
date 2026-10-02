//! Bounded channels between Smithay's compositor/event-loop thread and the OpenXR client thread.

use calloop::channel::{self, Channel, SyncSender};

use crate::panel::{PanelFrame, PanelPose, Ray3};

#[derive(Debug)]
pub enum XrInput {
    Ray { ray: Ray3, time_ms: u32 },
    Button { pressed: bool, time_ms: u32 },
}

#[derive(Debug)]
pub enum PanelUpdate {
    Frame {
        panel_id: u64,
        frame: PanelFrame,
        pose: PanelPose,
    },
    Removed {
        panel_id: u64,
    },
}

pub fn panel_channel() -> (SyncSender<PanelUpdate>, Channel<PanelUpdate>) {
    channel::sync_channel(4)
}

pub fn input_channel() -> (SyncSender<XrInput>, Channel<XrInput>) {
    channel::sync_channel(16)
}
