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
//! Crops are ONE ordered row-data list: a column takes the FIRST
//! spec whose biome gate and patch membership hit, so a later crop never
//! lands on an earlier one's cell BY CONSTRUCTION where their biomes overlap
//! (wheat ∩ carrots on Plains, carrots ∩ potatoes in Forests). Patch
//! membership is positional-RNG-pure per (seed, salt, anchor), so skipping a
//! later spec's evaluation never shifts any stream.
//!
//! Wild crops generate only in newly generated terrain (this is a gen-time
//! feature); enabling farming later does not retrofit explored sections.

use mod_sdk::*;

use crate::content::Content;

/// Max |offset| of a patch cell from its anchor; also the anchor scan reach.
const PATCH_REACH: i32 = 2;

/// The feature's write bounds for host-side admission: every patch cell is
/// the plant cell directly above its column's surface, nothing else.
pub(crate) const GEN_FILTER: GenFeatureFilter = GenFeatureFilter::surface_band(1, 1);

/// One wild crop's placement row. The slice ORDER is the priority order —
/// the first spec that hits a column owns it.
pub(crate) struct WildCropSpec {
    /// Positional-RNG salt for the crop's anchor/walk streams. Frozen:
    /// worldgen determinism depends on these exact literals.
    salt: u64,
    /// Random-walk step-count range (patch size/shape).
    patch: (i32, i32),
    /// The wild block the column plants.
    block: BlockId,
    /// Biome gate: the anchor chance for a column's biome, `None` outside
    /// the crop's biomes. Chances are balance data from the block row.
    chances: Vec<(u8, f32)>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WildPatchRow {
    priority: u16,
    /// Hex text keeps the frozen positional RNG stream visible to authors.
    salt: String,
    patch: (i32, i32),
    biomes: Vec<BiomeChance>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BiomeChance {
    biome: String,
    chance_denominator: u32,
}

/// Load the ordered patch rules once for each runtime instance. Biome names
/// resolve against the SDK's stable engine-biome vocabulary.
pub(crate) fn resolve_specs() -> Vec<WildCropSpec> {
    let mut rows = blocks_with_data_as::<WildPatchRow>(crate::keys::WILD_PATCH_DATA)
        .into_iter()
        .filter_map(|(block, row)| {
            let Some(salt) = u64::from_str_radix(row.salt.trim_start_matches("0x"), 16).ok() else {
                log(&format!(
                    "farming: invalid wild-patch salt '{}' for {block:?}",
                    row.salt
                ));
                return None;
            };
            if row.patch.0 < 1 || row.patch.0 > row.patch.1 {
                log(&format!("farming: invalid wild patch size for {block:?}"));
                return None;
            }
            let mut chances = Vec::new();
            for entry in row.biomes {
                let Some(biome) = biome::by_name(&entry.biome) else {
                    log(&format!(
                        "farming: unknown wild-patch biome '{}'",
                        entry.biome
                    ));
                    return None;
                };
                if entry.chance_denominator == 0 {
                    log(&format!("farming: zero wild-patch chance for {block:?}"));
                    return None;
                }
                chances.push((biome, 1.0 / entry.chance_denominator as f32));
            }
            Some((
                row.priority,
                WildCropSpec {
                    salt,
                    patch: row.patch,
                    block,
                    chances,
                },
            ))
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(priority, _)| *priority);
    rows.into_iter().map(|(_, spec)| spec).collect()
}

pub fn wild_patches(content: &Content, ctx: &GenCtx) -> Vec<GenWrite> {
    let specs = &content.wild_patches;
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
        // First spec whose biome gate + patch membership hit owns the cell.
        let Some(spec) = specs.iter().find(|spec| {
            spec.chances
                .iter()
                .find(|(id, _)| *id == biome)
                .map(|(_, chance)| *chance)
                .is_some_and(|chance| in_patch(ctx.seed(), spec.salt, chance, spec.patch, wx, wz))
        }) else {
            return;
        };
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
        writes.push(([wx, plant_y, wz], spec.block));
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

#[cfg(test)]
mod row_tests {
    use super::*;

    #[test]
    fn shipped_wild_patch_rows_preserve_priority_and_biome_gates() {
        let rows = pack_rows_with_data(
            include_str!("../pack/blocks.json"),
            "blocks",
            crate::keys::WILD_PATCH_DATA,
        );
        assert_eq!(rows.len(), 3);
        for (priority, (name, raw)) in rows.into_iter().enumerate() {
            let row: WildPatchRow = parse_row_data(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(usize::from(row.priority), priority);
            assert!(row.patch.0 >= 1 && row.patch.0 <= row.patch.1);
            assert!(u64::from_str_radix(row.salt.trim_start_matches("0x"), 16).is_ok());
            assert!(!row.biomes.is_empty());
            for entry in row.biomes {
                assert!(
                    biome::by_name(&entry.biome).is_some(),
                    "{name}: {}",
                    entry.biome
                );
                assert!(entry.chance_denominator > 0);
            }
        }
    }
}
