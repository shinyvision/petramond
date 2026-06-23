//! Per-block behaviour — a block's "class".
//!
//! Every block's data row ([`BlockDef`](super::definition::BlockDef)) points at
//! one `&'static dyn BlockBehavior`. Everything a block *does* (as opposed to what
//! it *is* — categorised by [`BlockTag`](super::BlockTag)) lives behind this
//! trait, so giving a block reactive behaviour is "write a behaviour, point its
//! row at it" — never a new `match` arm in the simulation. Most blocks use
//! [`INERT`] (every method defaulted); a block overrides only the hooks it needs.
//!
//! **One behaviour per file.** Each behaviour lives in its own module here and
//! re-exports its singleton below, so rows still read `behavior::LEAVES` while
//! `mod.rs` carries only the shared trait and the registry of behaviours. Adding
//! one is: add a file, add its `mod` + `pub use` line here, point the row at it.
//!
//! Behaviours act on the world through its PUBLIC api only — they never reach into
//! its internals — so a behaviour needing no privileged access (leaf decay) lives
//! here in `block`, while one that does (fluid flow, which drives the world
//! scheduler) can live in `world` and still implement this `block`-defined trait.

use crate::mathh::IVec3;
use crate::world::World;

mod inert;
mod leaves;

// The behaviour registry: one re-export per behaviour, so a data row points at a
// flat `&behavior::NAME`.
pub use inert::INERT;
pub use leaves::LEAVES;

/// The behaviour a block exhibits in the running world. Default methods make a
/// block inert; an implementor overrides only what it needs.
///
/// `Sync` because the behaviour singletons live in the `'static` block table,
/// which the gen and light worker threads read — so `dyn BlockBehavior` (and the
/// table holding it) is shareable across threads.
pub trait BlockBehavior: Sync {
    /// Whether this block receives random ticks — the probabilistic per-column
    /// callback the world fires at a few random cells each game tick (see
    /// `world::tick`). Gates both the dispatch and the per-chunk skip counter.
    fn has_random_tick(&self) -> bool {
        false
    }

    /// Run one random tick for this block at world voxel `pos`. Called only when
    /// [`has_random_tick`](Self::has_random_tick) is true; free to read and edit
    /// the world through its public api. Default: do nothing.
    fn random_tick(&self, world: &mut World, pos: IVec3) {
        let _ = (world, pos);
    }
}
