//! Compatibility shim — the worldgen god file has moved to `crate::worldgen`.
//!
//! Strata P0: `gen.rs` was split verbatim into `src/worldgen/`. This re-export
//! preserves the `crate::gen::*` / `llamacraft::gen::*` ABI used by
//! `src/worker.rs`, `worker_wasm`, and `src/app.rs` while the migration is in
//! flight. It is deleted at P4, when those call sites are repointed at
//! `crate::worldgen` directly.

pub use crate::worldgen::*;
