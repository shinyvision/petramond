//! Planted crops drawing their pest out of the wild.
//!
//! The wild population of an attracted species is deliberately thin (its
//! `mobs.json` row carries the species-wide spawn chance), so meeting one by
//! walking around is rare. A CULTIVATED patch is the other way to meet one:
//! while nobody is standing over it, a planted stand occasionally pulls one
//! in from nowhere, near enough to find the field and far enough away that
//! the player never watches it appear.
//!
//! The roll rides the crop's own RANDOM TICK, which is why there is no sweep
//! and no registry of fields: the engine already visits loaded crops at a
//! steady rate, so attraction scales with how much a player actually planted
//! and costs nothing on a world with no farms. Only rows whose [`CropDef`]
//! names an `attracts` species roll at all.
//!
//! Three gates stand between the roll and a mob, and each exists because
//! skipping it produces a specific bad spawn:
//!
//! - the PLAYER BAND ([`MIN_PLAYER_DIST`]) — an animal must never pop into
//!   existence in view; this mirrors the engine's own natural-spawn band.
//! - [`site_open`] — footing plus confinement, so nothing is dropped into a
//!   hole, buried in rock, or born captive inside the fence the player built
//!   around the very field that attracted it.
//! - the LOCAL HEADCOUNT ([`NEAR_MAX`]) — a field is an attraction, not a
//!   faucet; without it a big farm breeds an unbounded warren.

use mod_sdk::*;

use crate::content::Content;

/// One in this many random ticks on an attracting crop rolls an attempt.
/// A random tick reaches one given cell about every 70 s, so a modest field
/// rolls a few times a minute while the player is away from it.
const ATTRACT_CHANCE_IN: u64 = 40;
/// How far from the crop a spawn site may be picked (blocks) — the prompt's
/// ten: close enough that the newcomer finds the field it came for.
const SITE_RADIUS: i32 = 10;
/// Candidate sites tried per attempt before giving up (the next random tick
/// re-rolls, so a crowded neighbourhood simply waits).
const SITE_TRIES: u32 = 6;
/// Closest a spawn may appear to ANY player (blocks). Mirrors the engine's
/// own natural-spawn band: the animal is found, never witnessed arriving.
const MIN_PLAYER_DIST: f32 = 50.0;
/// How far a site may sit above or below the crop that attracted it. Without
/// it a column probe lands on whatever roof or overhang tops the column, and
/// the visitor arrives on the barn instead of in the field.
const SITE_DY: i32 = 4;
/// Radius the local headcount is taken over, and the count that stops
/// attracting. Two is a visit; a dozen is an infestation nobody asked for.
const NEAR_RADIUS: f32 = 16.0;
const NEAR_MAX: usize = 2;

/// One random tick on a cultivated crop: roll for a visitor.
pub fn on_random_tick(content: &Content, pos: [i32; 3], block: BlockId) {
    let Some((def, _)) = content.crop_stage(block) else {
        return;
    };
    let Some((species, kind)) = def.attracts else {
        return;
    };
    if !rng_u64(&def.attract_key).is_multiple_of(ATTRACT_CHANCE_IN) {
        return;
    }
    let field = [pos[0] as f32 + 0.5, pos[1] as f32, pos[2] as f32 + 0.5];
    let anchors: Vec<[f32; 3]> = players().iter().map(|p| p.state.pos).collect();
    // Standing over your own field is the common case, and no site within
    // reach of this crop could clear the band from there — settle it once,
    // against the crop, before paying for the headcount or any site probe.
    if anchors
        .iter()
        .any(|p| near(*p, field, MIN_PLAYER_DIST - SITE_RADIUS as f32))
    {
        return;
    }
    if mobs_in_radius(field, NEAR_RADIUS)
        .iter()
        .filter(|m| m.kind == kind)
        .count()
        >= NEAR_MAX
    {
        return;
    }
    for _ in 0..SITE_TRIES {
        let dx = (rng_u64(&def.attract_key) % (SITE_RADIUS as u64 * 2 + 1)) as i32 - SITE_RADIUS;
        let dz = (rng_u64(&def.attract_key) % (SITE_RADIUS as u64 * 2 + 1)) as i32 - SITE_RADIUS;
        let (x, z) = (pos[0] + dx, pos[2] + dz);
        // `None` = unloaded or not stream-final: no verdict, try elsewhere.
        let Some(ground) = surface_y_at([x, z]) else {
            continue;
        };
        let feet = [x, ground + 1, z];
        if (feet[1] - pos[1]).abs() > SITE_DY {
            continue;
        }
        let at = [x as f32 + 0.5, feet[1] as f32, z as f32 + 0.5];
        if anchors.iter().any(|p| near(*p, at, MIN_PLAYER_DIST)) {
            continue;
        }
        if !site_open(species, feet) {
            continue;
        }
        let yaw = (rng_u64(&def.attract_key) % 628) as f32 / 100.0;
        if spawn_mob_checked(species, at, yaw).is_some() {
            return;
        }
    }
}

fn near(a: [f32; 3], b: [f32; 3], range: f32) -> bool {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < range * range
}
