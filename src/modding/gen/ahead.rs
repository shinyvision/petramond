//! Section outputs a feature handed over ahead of their dispatch, applied when their section
//! generates instead of dispatching the feature there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

use petramond_worldgen::hooks::GenerationPlan;
use rustc_hash::FxHashMap;

/// Outputs one reply may hand over.
pub(super) const MAX_PER_REPLY: usize = 1024;
/// Outputs held per feature; past it the held ones are dropped, and their sections dispatch
/// the feature as usual.
const MAX_HELD: usize = 1 << 14;

#[derive(Default)]
pub(super) struct Ahead {
    any: AtomicBool,
    held: RwLock<FxHashMap<[i32; 3], GenerationPlan>>,
}

impl Ahead {
    pub(super) fn take(&self, section: [i32; 3]) -> Option<GenerationPlan> {
        if !self.any.load(Ordering::Relaxed) {
            return None;
        }
        if !self
            .held
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&section)
        {
            return None;
        }
        let mut held = self.held.write().unwrap_or_else(|e| e.into_inner());
        held.remove(&section)
    }

    pub(super) fn hold(&self, outputs: Vec<([i32; 3], GenerationPlan)>) {
        if outputs.is_empty() {
            return;
        }
        let mut held = self.held.write().unwrap_or_else(|e| e.into_inner());
        if held.len() + outputs.len() > MAX_HELD {
            held.clear();
        }
        held.extend(outputs);
        self.any.store(true, Ordering::Relaxed);
    }
}
