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
    let origin = ctx.origin_world();
    let oy = origin[1];
    // Each spec's patch coverage over this section, settled once for every column that asks.
    let mut coverage: Vec<Option<Coverage>> = vec![None; specs.len()];
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
        let Some(spec) = specs.iter().enumerate().find(|(i, spec)| {
            spec.chances
                .iter()
                .find(|(id, _)| *id == biome)
                .map(|(_, chance)| *chance)
                .is_some_and(|chance| {
                    coverage[*i]
                        .get_or_insert_with(|| {
                            Coverage::of(ctx.seed(), spec.salt, chance, spec.patch, origin)
                        })
                        .has(wx - origin[0], wz - origin[2])
                })
        }) else {
            return;
        };
        let spec = spec.1;
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

/// The columns of one section a spec's patches cover: every anchor within reach of the
/// section rolls once and its walk is marked, instead of every column re-rolling the 25
/// anchors around it. A column is covered exactly when one of those anchors' walks reaches
/// it, which is what the per-column test asked.
#[derive(Clone, Copy)]
struct Coverage([u16; 16]);

impl Coverage {
    fn of(seed: u32, salt: u64, chance: f32, (min, max): (i32, i32), origin: [i32; 3]) -> Coverage {
        let (ox, oz) = (origin[0], origin[2]);
        let mut rows = [0u16; 16];
        let mut mark = |x: i32, z: i32| {
            let (lx, lz) = (x - ox, z - oz);
            if (0..16).contains(&lx) && (0..16).contains(&lz) {
                rows[lz as usize] |= 1 << lx;
            }
        };
        for az in (oz - PATCH_REACH)..(oz + 16 + PATCH_REACH) {
            for ax in (ox - PATCH_REACH)..(ox + 16 + PATCH_REACH) {
                let mut rng = GenRng::positional(seed, salt, ax, 0, az);
                if !rng.chance(chance) {
                    continue;
                }
                mark(ax, az);
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
                    mark(cx, cz);
                }
            }
        }
        Coverage(rows)
    }

    #[inline]
    fn has(&self, lx: i32, lz: i32) -> bool {
        self.0[lz as usize] >> lx & 1 != 0
    }
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
