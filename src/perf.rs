//! Lightweight streaming-pipeline profiling counters.
//!
//! Each background stage (worldgen, light bake, mesh build) records its wall time and
//! invocation count into a global [`Phase`]. A profiler binary resets them, drives the
//! pipeline, then reads the snapshot — giving per-stage TOTAL CPU time and the unit
//! count, which is what reveals the cubic refactor's per-section multiplication. Cheap
//! relaxed atomics; the recording is off the hot path's critical section.

use std::sync::atomic::{AtomicU64, Ordering};

pub struct Phase {
    ns: AtomicU64,
    count: AtomicU64,
}

impl Phase {
    const fn new() -> Self {
        Self {
            ns: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn record(&self, ns: u64) {
        self.ns.fetch_add(ns, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// `(total_ns, count)`.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.ns.load(Ordering::Relaxed),
            self.count.load(Ordering::Relaxed),
        )
    }

    pub fn reset(&self) {
        self.ns.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
    }
}

/// Per-column shared-data generation (the 2-D noise window).
pub static GEN_COLUMN: Phase = Phase::new();
/// Per-section terrain + feature fill.
pub static GEN_SECTION: Phase = Phase::new();
/// Per-section light bake (skylight + block-light flood).
pub static LIGHT: Phase = Phase::new();
/// Per-section mesh build.
pub static MESH: Phase = Phase::new();

pub fn reset_all() {
    GEN_COLUMN.reset();
    GEN_SECTION.reset();
    LIGHT.reset();
    MESH.reset();
}
