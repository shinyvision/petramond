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

mod dirt;
mod grass;
mod inert;
mod leaves;

// The behaviour registry: one re-export per behaviour, so a data row points at a
// flat `&behavior::NAME`. Behaviours that reach into world internals live under
// `world` (they can't from here) but are still listed here for one-stop reading.
pub use crate::world::door::DOOR;
pub use crate::world::fragile::FRAGILE;
pub use crate::world::sapling::SAPLING;
pub use crate::world::water::WATER;
pub use dirt::DIRT;
pub use grass::GRASS;
pub use inert::INERT;
pub use leaves::LEAVES;

/// The behaviour a block exhibits in the running world. Default methods make a
/// block inert; an implementor overrides only what it needs.
///
/// `Sync` because the behaviour singletons live in the `'static` block table,
/// which the gen and light worker threads read — so `dyn BlockBehavior` (and the
/// table holding it) is shareable across threads.
pub trait BlockBehavior: Sync {
    /// The stable data-file name of this behaviour (`"inert"`, `"leaves"`, …) —
    /// what a block row's `behavior` field in `blocks.json` references. Each
    /// singleton returns its own literal; [`by_name`] is the inverse.
    fn key(&self) -> &'static str;

    /// Whether this block receives random ticks — the probabilistic per-section
    /// callback the world fires at a few random cells each game tick (see
    /// `world::tick`). Gates both the dispatch and the per-section skip counter.
    fn has_random_tick(&self) -> bool {
        false
    }

    /// Run one random tick for this block at world voxel `pos`. Called only when
    /// [`has_random_tick`](Self::has_random_tick) is true; free to read and edit
    /// the world through its public api. Default: do nothing.
    fn random_tick(&self, world: &mut World, pos: IVec3) {
        let _ = (world, pos);
    }

    /// React to a neighbour change — the ANNOUNCE phase of a block update, fired
    /// for a cell at or beside a change. Free to schedule a future
    /// [`scheduled_tick`](Self::scheduled_tick) or edit the world. Default: do
    /// nothing. (Water schedules its flow check here.)
    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        let _ = (world, pos);
    }

    /// Run a scheduled tick previously requested for this cell — the EXECUTE phase,
    /// `delay` ticks after it was scheduled. Default: do nothing. (Water runs its
    /// flow check here.)
    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        let _ = (world, pos);
    }
}

/// Resolve a behaviour's data-file name (a `blocks.json` row's `behavior` field)
/// to its singleton — the inverse of [`BlockBehavior::key`]. One arm per
/// registered behaviour above; a new behaviour joins the data files by adding
/// its arm here.
pub fn by_name(name: &str) -> Option<&'static dyn BlockBehavior> {
    Some(match name {
        "inert" => &INERT,
        "grass" => &GRASS,
        "dirt" => &DIRT,
        "leaves" => &LEAVES,
        "water" => &WATER,
        "fragile" => &FRAGILE,
        "sapling" => &SAPLING,
        "door" => &DOOR,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behavior_names_round_trip() {
        for name in [
            "inert", "grass", "dirt", "leaves", "water", "fragile", "sapling", "door",
        ] {
            let b = by_name(name).unwrap_or_else(|| panic!("unregistered behavior '{name}'"));
            assert_eq!(b.key(), name, "key() must be the inverse of by_name()");
        }
        assert!(by_name("bogus").is_none());
    }
}
