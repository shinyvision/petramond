//! Rabbit hop gait. Pack policy on top of the engine's generic velocity access, with no gait
//! vocabulary on the engine side. A grounded rabbit whose brain is walking it somewhere gets a
//! vertical launch, the wish carries the arc, and the engine's walking-launch rules keep the walk
//! clip playing and phase-locked through it. `mob_drive_vertical` plus the snapshot's
//! `vel`/`on_ground` is the whole seam.
//!
//! Runs every tick right after the mobs move. The launch decision reads this tick's landing and
//! the drive intent it issues is consumed by the next tick's integration, so the rabbit is on the
//! ground for exactly one tick between hops. The walk clip is tuned to that bounce cadence.

use mod_sdk::*;

use crate::content::Content;

const HOP_SPEED: f32 = 4.6;
const RANGE: f32 = 96.0;
/// Consecutive launches from the same neighbourhood (anchor cell ± 1) before
/// the rabbit CALMS DOWN and walks instead: a hop's ~1-block stride cannot
/// resolve a pocket smaller than itself, and walking can. Honest travel
/// re-anchors within a hop or two, so it never accumulates.
const STALL_LAUNCHES: i64 = 8;
const CALM_TICKS: i64 = 60;
/// Ticks without a launch after which the launch-site memory is FORGOTTEN.
/// A trap is CONTINUOUS launching from one neighbourhood (a launch every
/// ~10 ticks, no pauses); separate short wander legs around the same spot
/// have idle gaps between them and must not accumulate toward a calm-down.
const STALL_MEMORY_GAP: i64 = 40;

const ANCHOR: &str = "farming:hop_anchor";
const STALL: &str = "farming:hop_stall";
const CALM_UNTIL: &str = "farming:hop_calm_until";
const LAST_LAUNCH: &str = "farming:hop_last";

fn packed_cell(pos: [f64; 3]) -> i64 {
    let (x, z) = (pos[0].floor() as i64, pos[2].floor() as i64);
    (x << 32) | (z & 0xffff_ffff)
}

fn cell_near(a: i64, b: i64) -> bool {
    let (ax, az) = (a >> 32, (a as i32 as i64));
    let (bx, bz) = (b >> 32, (b as i32 as i64));
    (ax - bx).abs() <= 1 && (az - bz).abs() <= 1
}

/// Launches every grounded rabbit that's walking somewhere near a player. A rabbit that keeps
/// launching from the same spot walks for a while instead (tight spots get walked, open ground
/// gets hopped).
/// The gate is the snapshot's `moving`, never velocity, since hopping comes from navigating: a
/// rabbit shoved by a player or knocked back slides instead of bouncing, and one just slowed by a
/// wall or a crowd still hops.
/// One sweep covers every player's range and visits each rabbit once, so overlapping ranges don't
/// repeat host calls or double-count a launch toward a stall.
pub fn on_tick(content: &Content) {
    let tick = current_tick() as i64;
    let anchors: Vec<[f64; 3]> = players().iter().map(|p| p.state.pos).collect();
    for snap in mobs_near_any_of(&anchors, RANGE, &[content.rabbit]) {
        if !snap.on_ground || !snap.moving {
            continue;
        }
        let Some(tags) = mob_tags_get(snap.id) else {
            continue;
        };
        let tag = |key: &str| {
            tags.iter().find_map(|(k, v)| match v {
                MobTagValue::I64(i) if k == key => Some(*i),
                _ => None,
            })
        };
        if tag(CALM_UNTIL).is_some_and(|until| until > tick) {
            continue;
        }
        let here = packed_cell(snap.pos);
        let continuous = tag(LAST_LAUNCH).is_some_and(|last| tick - last <= STALL_MEMORY_GAP);
        mob_tag_set(snap.id, LAST_LAUNCH, MobTagValue::I64(tick));
        match tag(ANCHOR).filter(|_| continuous) {
            Some(anchor) if cell_near(anchor, here) => {
                let stall = tag(STALL).unwrap_or(0) + 1;
                if stall >= STALL_LAUNCHES {
                    mob_tag_set(snap.id, CALM_UNTIL, MobTagValue::I64(tick + CALM_TICKS));
                    mob_tag_delete(snap.id, ANCHOR);
                    mob_tag_delete(snap.id, STALL);
                    continue;
                }
                mob_tag_set(snap.id, STALL, MobTagValue::I64(stall));
            }
            _ => {
                mob_tag_set(snap.id, ANCHOR, MobTagValue::I64(here));
                if tag(STALL).is_some() {
                    mob_tag_delete(snap.id, STALL);
                }
            }
        }
        mob_drive_vertical(snap.id, HOP_SPEED, true);
    }
}
