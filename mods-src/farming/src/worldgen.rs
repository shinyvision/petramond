//! Wild crop patches: a deterministic additive feature after the Trees stage.
//!
//! DESIGN. Patch placement is a pure function of (world seed, position,
//! biome, final local surface facts) via positional RNG only — no visit
//! order, no host RNG stream. Every column asks: "does any patch ANCHOR
//! within reach cover me?" Anchor existence and the patch's random-walk
//! shape derive solely from (seed, anchor coords), so every section that
//! touches a patch derives the same cells independently — seam-safe by
//! construction. Column facts (surface height, biome, grass root, clear
//! cell) are then validated PER PLANT COLUMN with the section's own data,
//! clipping patches naturally at biome edges and obstacles.
//!
//! Wheat is decided before carrots in the same pass, and a carrot cell is
//! skipped wherever wheat would plant, so the two never overwrite one
//! another where their biomes overlap (Plains).
//!
//! Wild crops generate only in newly generated terrain (this is a gen-time
//! feature); enabling farming later does not retrofit explored sections.

use mod_sdk::*;

use crate::content::Content;

const WHEAT_SALT: u64 = 0x00FA_57EA_7000_0001;
const CARROT_SALT: u64 = 0x00FA_57EA_7000_0002;

/// Patch anchor probability per eligible column. Balance data: tuned (map
/// inspection over several seeds) so purposeful exploration of an eligible
/// biome reveals a patch within roughly three to five minutes while patches
/// still feel found, not ubiquitous — about one patch per ~150x150 blocks of
/// eligible terrain.
const WHEAT_ANCHOR_CHANCE: f32 = 1.0 / 22000.0;
const CARROT_ANCHOR_CHANCE: f32 = 1.0 / 26000.0;

/// Patch sizes (random-walk step counts — revisits make real patches
/// slightly smaller and irregular, which is the intent).
const WHEAT_PATCH: (i32, i32) = (4, 8);
const CARROT_PATCH: (i32, i32) = (3, 6);

/// Max |offset| of a patch cell from its anchor; also the anchor scan reach.
const PATCH_REACH: i32 = 2;

fn wheat_biome(b: u8) -> bool {
    b == biome::PLAINS || b == biome::SAVANNA
}

fn carrot_biome(b: u8) -> bool {
    b == biome::PLAINS || b == biome::FOREST
}

pub fn wild_patches(content: &Content, ctx: &GenCtx) -> Vec<GenWrite> {
    let mut writes = Vec::new();
    let oy = ctx.origin_world()[1];
    ctx.for_each_origin(0, |wx, wz| {
        let Some(surface) = ctx.surface_y(wx, wz) else {
            return;
        };
        // Rooted only on ordinary grass above the waterline.
        if surface <= ctx.sea_level() {
            return;
        }
        let plant_y = surface + 1;
        // Only the section that owns the PLANT cell may emit it; requiring
        // the root cell in the same section keeps the grass check readable
        // (the rare surface-at-section-top column simply grows no patch —
        // deterministically, on every side of the seam).
        if plant_y < oy + 1 || plant_y >= oy + 16 {
            return;
        }
        let Some(biome) = ctx.biome(wx, wz) else {
            return;
        };
        let wheat_here = wheat_biome(biome)
            && in_patch(
                ctx.seed(),
                WHEAT_SALT,
                WHEAT_ANCHOR_CHANCE,
                WHEAT_PATCH,
                wx,
                wz,
            );
        let carrot_here = !wheat_here
            && carrot_biome(biome)
            && in_patch(
                ctx.seed(),
                CARROT_SALT,
                CARROT_ANCHOR_CHANCE,
                CARROT_PATCH,
                wx,
                wz,
            );
        if !wheat_here && !carrot_here {
            return;
        }
        // Final local surface facts: an ordinary grass root, and a plant
        // cell that is air or replaceable ground vegetation — never a tree,
        // solid block, other crop, or structure.
        if ctx.block([wx, surface, wz]) != Some(content.grass) {
            return;
        }
        match ctx.block([wx, plant_y, wz]) {
            Some(BlockId::AIR) => {}
            Some(b) if content.is_clearable_cover(b) => {}
            _ => return,
        }
        let block = if wheat_here {
            content.wild_wheat
        } else {
            content.wild_carrots
        };
        writes.push(([wx, plant_y, wz], block));
    });
    writes
}

/// Whether any patch anchor within reach covers `(wx, wz)`. Anchor rolls and
/// walk shapes are positional-RNG-pure per (seed, salt, anchor), so every
/// caller — whichever section it generates — computes the same membership.
fn in_patch(seed: u32, salt: u64, chance: f32, (min, max): (i32, i32), wx: i32, wz: i32) -> bool {
    for az in (wz - PATCH_REACH)..=(wz + PATCH_REACH) {
        for ax in (wx - PATCH_REACH)..=(wx + PATCH_REACH) {
            let mut rng = GenRng::positional(seed, salt, ax, 0, az);
            if !rng.chance(chance) {
                continue;
            }
            if (ax, az) == (wx, wz) {
                return true;
            }
            // The irregular connected shape: a random walk from the anchor,
            // clamped to the reach box.
            let steps = rng.next_i32(min, max);
            let (mut cx, mut cz) = (ax, az);
            for _ in 1..steps {
                let (dx, dz) = match rng.next_u64() % 4 {
                    0 => (1, 0),
                    1 => (-1, 0),
                    2 => (0, 1),
                    _ => (0, -1),
                };
                let (nx, nz) = (cx + dx, cz + dz);
                if (nx - ax).abs() > PATCH_REACH || (nz - az).abs() > PATCH_REACH {
                    continue;
                }
                (cx, cz) = (nx, nz);
                if (cx, cz) == (wx, wz) {
                    return true;
                }
            }
        }
    }
    false
}
