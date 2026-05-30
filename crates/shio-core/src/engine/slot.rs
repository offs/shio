use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::EngineState;

pub(super) struct ActiveSlot {
    state: Arc<EngineState>,
}

impl ActiveSlot {
    pub(super) fn new(state: Arc<EngineState>) -> Self {
        state.active_count.fetch_add(1, Ordering::Relaxed);
        Self { state }
    }
}

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        self.state.active_count.fetch_sub(1, Ordering::Relaxed);
        self.state.slot_notify.notify_one();
    }
}
