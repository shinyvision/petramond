//! Per-block behaviour — a block's "class".
//!
//! Every block's data row (`BlockDef`) points at
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

use crate::block::Block;
use crate::mathh::IVec3;
use crate::world::data::WorldData;

mod dirt;
mod grass;
mod inert;
mod leaves;
mod wasm;

pub use wasm::ModBlockHook;

// The behaviour registry: one re-export per behaviour, so a data row points at a
// flat `&behavior::NAME`. Behaviours that reach into world internals live under
// `world` (they can't from here) but are still listed here for one-stop reading.
pub use dirt::DIRT;
pub use grass::GRASS;
pub use inert::INERT;
pub use leaves::LEAVES;
// The decay flood's reach is part of the leaf-support contract: worldgen
// canopy placement bounds its trunk-connectivity flood by the same number, so
// generated leaves can never sit outside the distance the decay rule enforces.
pub use leaves::MAX_LOG_DISTANCE;

/// The world surface a behaviour acts through. `Deref`s to [`WorldData`]
/// (every read and data-half mutation), plus the few orchestrated mutations a
/// behaviour may trigger. Implemented by the engine's `World`; behaviours act
/// on the world through this PUBLIC api only.
pub trait BehaviorWorld: std::ops::DerefMut<Target = WorldData> {
    /// Set a block through the full edit path (invalidation, replication).
    fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool;
    /// Break a block as the simulation (break burst + natural drops).
    fn break_block_naturally(&mut self, pos: IVec3);
}

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
    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        let _ = (world, pos);
    }

    /// React to a neighbour change — the ANNOUNCE phase of a block update, fired
    /// for a cell at or beside a change. Free to schedule a future
    /// [`scheduled_tick`](Self::scheduled_tick) or edit the world. Default: do
    /// nothing. (Water schedules its flow check here.)
    fn neighbor_update(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        let _ = (world, pos);
    }

    /// Run a scheduled tick previously requested for this cell — the EXECUTE phase,
    /// `delay` ticks after it was scheduled. Default: do nothing. (Water runs its
    /// flow check here.)
    fn scheduled_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        let _ = (world, pos);
    }
}

/// Resolve a behaviour's data-file name (a `blocks.json` row's `behavior` field)
/// to its singleton — the inverse of [`BlockBehavior::key`]. One arm per
/// registered engine behaviour above; a new engine behaviour joins the data
/// files by adding its arm here. A NAMESPACED key (`mod_id:name`) resolves to
/// a per-key `wasm::WasmBehavior` singleton that forwards every hook to the
/// owning mod (see that module) — so a pack gives its block reactive behaviour
/// by naming a key here and registering it via `RegisterBlockBehavior`.
pub fn by_name(name: &str) -> Option<&'static dyn BlockBehavior> {
    Some(match name {
        "inert" => &INERT,
        "grass" => &GRASS,
        "dirt" => &DIRT,
        "leaves" => &LEAVES,
        "water" => &WATER_HOOK,
        "fragile" => &FRAGILE_HOOK,
        "sapling" => &SAPLING_HOOK,
        "door" => &DOOR_HOOK,
        // The reserved engine namespace never dispatches to a mod.
        _ if crate::registry::namespace(name)
            .is_some_and(|ns| ns != crate::registry::ENGINE_NAMESPACE) =>
        {
            wasm::interned(name)
        }
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

/// A behaviour whose LOGIC lives above the data layer (fluid flow, fragile
/// support, sapling growth, doors — they drive the scheduler, drops, or
/// worldgen features). The data layer knows its static FACTS (key, random-tick
/// participation) so tables and gates stay correct; the engine's tick dispatch
/// resolves the key against its own registry BEFORE falling back to this
/// object, so these hook bodies are unreachable in a correctly-wired engine.
pub struct EngineHook {
    key: &'static str,
    random_tick: bool,
}

pub static WATER_HOOK: EngineHook = EngineHook { key: "water", random_tick: false };
pub static FRAGILE_HOOK: EngineHook = EngineHook { key: "fragile", random_tick: false };
pub static SAPLING_HOOK: EngineHook = EngineHook { key: "sapling", random_tick: true };
pub static DOOR_HOOK: EngineHook = EngineHook { key: "door", random_tick: false };

impl BlockBehavior for EngineHook {
    fn key(&self) -> &'static str {
        self.key
    }

    fn has_random_tick(&self) -> bool {
        self.random_tick
    }

    fn random_tick(&self, _world: &mut dyn BehaviorWorld, _pos: IVec3) {
        debug_assert!(false, "engine behaviour '{}' must dispatch through the engine registry", self.key);
    }

    fn neighbor_update(&self, _world: &mut dyn BehaviorWorld, _pos: IVec3) {
        debug_assert!(false, "engine behaviour '{}' must dispatch through the engine registry", self.key);
    }

    fn scheduled_tick(&self, _world: &mut dyn BehaviorWorld, _pos: IVec3) {
        debug_assert!(false, "engine behaviour '{}' must dispatch through the engine registry", self.key);
    }
}

/// Pub paths to the per-behaviour `test_exports` shims (the behaviour modules
/// themselves stay private). Test-support builds only.
#[cfg(any(test, feature = "test-support"))]
pub mod test_shims {
    pub use super::dirt::test_exports as dirt;
    pub use super::grass::test_exports as grass;
    pub use super::leaves::test_exports as leaves;
    pub use super::wasm::test_exports as wasm;
}
