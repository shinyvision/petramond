//! Wild crop patches. Runs after Trees.
//!
//! Only seed, position, biome and surface facts matter - positional RNG, no visit order, no host
//! RNG stream. Anchor and shape both come from seed + anchor coords, so neighboring sections land
//! on the same cells without coordinating. Seam-safe.
//!
//! Height, biome, grass root and clear-cell checks use the local section's data per plant column,
//! so patches get clipped at biome edges and obstacles.
//!
//! Crops are one ordered list, first match wins. Wheat/carrots overlap on Plains and
//! carrots/potatoes in Forests - the earlier crop keeps the cell. Membership checks are
//! positional-RNG-pure, so skipping a spec never shifts the RNG stream.
//!
//! Gen-time only. Turning farming on later doesn't backfill crops into terrain you already
//! explored.

use mod_sdk::*;

use crate::content::Content;

const PATCH_REACH: i32 = 2;

pub(crate) const GEN_FILTER: GenFeatureFilter = GenFeatureFilter::surface_band(1, 1);

pub(crate) struct WildCropSpec {
    salt: u64,
    patch: (i32, i32),
    block: BlockId,
    chances: Vec<(u8, f32)>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WildPatchRow {
    priority: u16,
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
        if surface <= ctx.sea_level() {
            return;
        }
        let plant_y = surface + 1;
        if plant_y < oy + 1 || plant_y >= oy + 16 {
            return;
        }
        let Some(biome) = ctx.biome(wx, wz) else {
            return;
        };
        let Some(spec) = specs.iter().find(|spec| {
            spec.chances
                .iter()
                .find(|(id, _)| *id == biome)
                .map(|(_, chance)| *chance)
                .is_some_and(|chance| in_patch(ctx.seed(), spec.salt, chance, spec.patch, wx, wz))
        }) else {
            return;
        };
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
