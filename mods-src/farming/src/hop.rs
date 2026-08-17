//! The rabbit's HOP GAIT — pack policy over the engine's generic velocity
//! access, no gait vocabulary engine-side: a grounded rabbit whose brain is
//! walking it somewhere gets a vertical launch, the wish carries the arc, and
//! the engine's generic walking-launch rules keep the walk clip playing and
//! phase-locked through it (WIKI-free summary: `mob_drive_vertical` + the
//! snapshot's `vel`/`on_ground` are the whole seam).
//!
//! Runs every tick right after the mobs move: the launch decision reads this
//! tick's landing, and the drive intent it issues is consumed by the next
//! tick's integration — the rabbit touches ground for exactly one tick
//! between hops, which is the bounce cadence the walk clip is tuned to.

use mod_sdk::*;

use crate::content::Content;

/// Upward launch speed (m/s). Height = v²/2g ≈ 0.48 blocks, airtime ≈ 0.42 s
/// — a bounce that clears nothing the navigator doesn't already route
/// (deliberately below the one-block step the engine's own jump is sized
/// for). Balance data.
const HOP_SPEED: f32 = 4.6;
/// How far around each player rabbits are swept for launches. Matches the
/// mob simulation's own player-reactive scale; a rabbit far beyond it walks
/// flat until a player is near enough to watch it.
const RANGE: f32 = 96.0;
/// Consecutive launches from the same neighbourhood (anchor cell ± 1) before
/// the rabbit CALMS DOWN and walks instead: a hop's ~1-block stride cannot
/// resolve a pocket smaller than itself, and walking can. Honest travel
/// re-anchors within a hop or two, so it never accumulates.
const STALL_LAUNCHES: i64 = 8;
/// How long a calmed rabbit walks (ticks) before hopping resumes — enough to
/// thread any local pocket at full walk speed with full steering (7+ blocks
/// covered), short enough that a watcher reads a brief walk, not a rabbit
/// that stopped hopping.
const CALM_TICKS: i64 = 60;
/// Ticks without a launch after which the launch-site memory is FORGOTTEN.
/// A trap is CONTINUOUS launching from one neighbourhood (a launch every
/// ~10 ticks, no pauses); separate short wander legs around the same spot
/// have idle gaps between them and must never accumulate toward a calm-down
/// — measured 2026-08-17: cross-leg accumulation tripped one false calm
/// that suppressed 5 s of honest open-ground hopping (a third of the
/// session's walking).
const STALL_MEMORY_GAP: i64 = 40;

/// The rabbit's launch-site memory, per mob on its tag map (per doctrine:
/// per-mob state is tags, never a guest-side table).
const ANCHOR: &str = "farming:hop_anchor";
const STALL: &str = "farming:hop_stall";
const CALM_UNTIL: &str = "farming:hop_calm_until";
const LAST_LAUNCH: &str = "farming:hop_last";

fn packed_cell(pos: [f32; 3]) -> i64 {
    let (x, z) = (pos[0].floor() as i64, pos[2].floor() as i64);
    (x << 32) | (z & 0xffff_ffff)
}

fn cell_near(a: i64, b: i64) -> bool {
    let (ax, az) = (a >> 32, (a as i32 as i64));
    let (bx, bz) = (b >> 32, (b as i32 as i64));
    (ax - bx).abs() <= 1 && (az - bz).abs() <= 1
}

/// Launch every grounded rabbit whose brain is WALKING it somewhere near a
/// player — unless it has been launching from the same neighbourhood over
/// and over, in which case it walks for a while (tight spots are walked,
/// open ground is hopped). The gate is the snapshot's `moving` — the
/// deliberate-locomotion fact — never velocity: hopping stems from
/// NAVIGATING, so a rabbit shoved by a player or bowled over by knockback
/// slides like any other body instead of bouncing, and a navigating rabbit
/// momentarily slowed by a wall or a crowd still hops. Duplicate sweeps of
/// one rabbit from overlapping player ranges are harmless: the drive intent
/// is a latch and both writes carry the same value, and the stall
/// bookkeeping is keyed to the unchanged landing cell.
pub fn on_tick(content: &Content) {
    let tick = current_tick() as i64;
    for player in players() {
        for snap in mobs_in_radius(player.state.pos, RANGE) {
            if snap.kind != content.rabbit || !snap.on_ground || !snap.moving {
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
                continue; // walking it out
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
}
