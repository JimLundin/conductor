#![expect(
    clippy::disallowed_types,
    reason = "Registering an adapter must be atomic with cancelling its concurrently executing launch"
)]

use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Control(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    cancelled: bool,
    instance: Option<runtime::Instance>,
}

impl Control {
    pub fn record(&self, instance: runtime::Instance) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        if state.cancelled {
            return false;
        }
        state.instance = Some(instance);
        true
    }

    pub fn cancel(&self) -> Option<runtime::Instance> {
        let mut state = self.0.lock().ok()?;
        state.cancelled = true;
        state.instance.take()
    }
}
