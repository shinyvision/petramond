//! Farmland: the shared hydration probe and the dry/wet visual
//! reconciliation.
//!
//! HYDRATION RULE: farmland is hydrated when at least one water block exists
//! at the SAME world Y within a square horizontal radius of 4 (|dx| ≤ 4,
//! |dz| ≤ 4). Source, flowing, and falling water all count — the block query
//! deliberately cannot distinguish them, which makes routed channels and
//! diverted streams work as irrigation.
//!
//! The wet/dry BLOCK is only an appearance and reconciles on this block's own
//! RANDOM TICKS — bounded, local, and deliberately unhurried (per Rachel:
//! farmland getting wet is random-tick based, like grass spread). A crop
//! growth attempt always probes the REAL hydration, so a stale texture can
//! never grow or pause a crop; only the look catches up lazily.

use mod_sdk::*;

use crate::content::Content;

pub const HYDRATION_RADIUS: i32 = 4;

/// A hydration probe's verdict. `Unknown` = no water found among readable
/// cells but some in-reach cell is unloaded/streaming — callers must retry
/// later, never treat it as dry-forever or hydrated.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Hydration {
    Hydrated,
    Dry,
    Unknown,
}

/// Probe the hydration rule around a farmland cell: one batched read of the
/// same-Y square, bounded and local (never a whole-world scan).
pub fn probe(content: &Content, pos: [i32; 3]) -> Hydration {
    let mut cells = Vec::with_capacity((HYDRATION_RADIUS as usize * 2 + 1).pow(2) - 1);
    for dz in -HYDRATION_RADIUS..=HYDRATION_RADIUS {
        for dx in -HYDRATION_RADIUS..=HYDRATION_RADIUS {
            if dx == 0 && dz == 0 {
                continue;
            }
            cells.push([pos[0] + dx, pos[1], pos[2] + dz]);
        }
    }
    let mut any_unknown = false;
    for got in get_blocks(cells) {
        match got {
            Some(b) if b == content.water => return Hydration::Hydrated,
            Some(_) => {}
            None => any_unknown = true,
        }
    }
    if any_unknown {
        Hydration::Unknown
    } else {
        Hydration::Dry
    }
}

/// A player built something in the cell directly above farmland. Anything
/// other than one of this pack's crops presses the soil back to ordinary
/// dirt — farmland only exists under open sky or a planted crop.
pub fn on_block_placed_above(content: &Content, pos: [i32; 3], block: BlockId) {
    if content.crop_stage(block).is_some() {
        return;
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    match get_block(below) {
        Some(b) if content.is_farmland(b) => {
            set_block(below, content.dirt);
        }
        _ => {}
    }
}

/// Cell-KV key for the consecutive-cropless-random-ticks counter.
const IDLE_KEY: &str = "farming:idle";
/// Random ticks in a row without a crop above before farmland (wet OR dry)
/// presses back to dirt — untended soil doesn't stay tilled.
const IDLE_REVERT_TICKS: u8 = 3;

/// Farmland block hooks: only random ticks do work — the idle-decay count
/// and the wet/dry visual reconcile. Neighbor updates and scheduled ticks
/// are deliberately unused.
pub fn on_hook(content: &Content, kind: BlockHookKind, pos: [i32; 3]) {
    match kind {
        BlockHookKind::RandomTick => random_tick(content, pos),
        BlockHookKind::NeighborUpdate | BlockHookKind::ScheduledTick => {}
    }
}

fn random_tick(content: &Content, pos: [i32; 3]) {
    let Some(current) = get_block(pos) else {
        return;
    };
    if !content.is_farmland(current) {
        return;
    }
    // Idle decay: a crop above resets the count; an empty (readable) cell
    // above counts one, and the third consecutive count reverts to dirt.
    // A streaming read neither counts nor resets.
    let mut carry_idle = None;
    match get_block([pos[0], pos[1] + 1, pos[2]]) {
        None => {}
        Some(above) if content.crop_stage(above).is_some() => {
            section_kv_delete(pos, IDLE_KEY);
        }
        Some(_) => {
            let idle = section_kv_get(pos, IDLE_KEY)
                .and_then(|b| b.first().copied())
                .unwrap_or(0)
                + 1;
            if idle >= IDLE_REVERT_TICKS {
                set_block(pos, content.dirt);
                return;
            }
            carry_idle = Some(idle);
        }
    }
    // Visual reconcile: swap to match REAL hydration. `Unknown` changes
    // nothing (the next random tick retries); a swap goes through the
    // ordinary block write so neighbors see the update.
    let want = match probe(content, pos) {
        Hydration::Hydrated => content.farmland_wet,
        Hydration::Dry => content.farmland_dry,
        Hydration::Unknown => current,
    };
    if current != want {
        set_block(pos, want);
    }
    // Written AFTER the possible swap — a block write clears the cell's KV,
    // and the count must survive the wet/dry flip.
    if let Some(idle) = carry_idle {
        section_kv_set(pos, IDLE_KEY, vec![idle]);
    }
}
