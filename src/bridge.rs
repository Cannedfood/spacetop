//! Latest panel updates and bounded input between the compositor and OpenXR threads.

use calloop::channel::{self, Channel, SyncSender};
use smithay::backend::allocator::dmabuf::Dmabuf;
#[cfg(test)]
use std::sync::mpsc::TryRecvError;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use crate::panel::{PanelGeometry, PanelLimits, Ray3};

#[derive(Debug)]
pub enum XrInput {
    Ray {
        ray: Ray3,
        time_ms: u32,
    },
    Button {
        pressed: bool,
        time_ms: u32,
    },
    GpuDevice {
        render_node: std::path::PathBuf,
        limits: PanelLimits,
    },
    FatalError {
        message: String,
    },
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

pub struct PanelSender(Arc<Mutex<BTreeMap<u64, PanelUpdate>>>);

pub struct PanelReceiver(Arc<Mutex<BTreeMap<u64, PanelUpdate>>>);

impl PanelSender {
    pub fn publish(&self, update: PanelUpdate) {
        let panel_id = match &update {
            PanelUpdate::GpuFrame { panel_id, .. } | PanelUpdate::Removed { panel_id } => *panel_id,
        };
        self.0
            .lock()
            .expect("panel updates lock")
            .insert(panel_id, update);
    }
}

impl PanelReceiver {
    pub fn drain(&self) -> Vec<PanelUpdate> {
        std::mem::take(&mut *self.0.lock().expect("panel updates lock"))
            .into_values()
            .collect()
    }

    #[cfg(test)]
    pub fn try_recv(&self) -> Result<PanelUpdate, TryRecvError> {
        self.0
            .lock()
            .expect("panel updates lock")
            .pop_first()
            .map(|(_, update)| update)
            .ok_or(TryRecvError::Empty)
    }
}

pub fn panel_channel() -> (PanelSender, PanelReceiver) {
    let pending = Arc::new(Mutex::new(BTreeMap::new()));
    (PanelSender(pending.clone()), PanelReceiver(pending))
}

pub fn input_channel() -> (SyncSender<XrInput>, Channel<XrInput>) {
    channel::sync_channel(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removals_are_not_lost_when_many_windows_update() {
        let (sender, receiver) = panel_channel();
        for panel_id in 0..32 {
            sender.publish(PanelUpdate::Removed { panel_id });
            sender.publish(PanelUpdate::Removed { panel_id });
        }
        assert_eq!(receiver.drain().len(), 32);
        assert!(receiver.drain().is_empty());
    }
}
