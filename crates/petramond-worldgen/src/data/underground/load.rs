//! The underground-biome catalog LOADER: the raw JSON file shape, the value
//! ranges every field is validated against, and the conversion into the
//! resolved table the queries read.
//!
//! Kept apart from the vocabulary it produces, the way `block/load.rs` is kept
//! apart from `block.rs`.

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    /// Absent in a layer that only adds fluid rows.
    #[serde(default)]
    underground_biomes: Vec<RawUndergroundBiome>,
    #[serde(default)]
    fluid_pools: Vec<fluids::RawPool>,
    #[serde(default)]
    fluid_falls: Vec<fluids::RawFall>,
}

/// A `shell` past ~2 stops reading as a LINING and starts painting most of the
/// biome's rock (measured: 4.0 lines over half the sampled volume), and it
/// widens the skip mask for the whole world, so it costs every generating
/// thread. Legal, deliberately — some biome may want cathedral-thick walls —
/// but it is a heavy choice, not a free one.
const SHELL_MAX: f64 = 8.0;

const BLEND_FIELD_MAX: f64 = 0.5;

const BLEND_Y_MAX: f64 = 64.0;

pub(super) fn parse_layers(texts: &[&str]) -> Result<UndergroundBiomes, String> {
    let mut fluid_layers = Vec::with_capacity(texts.len());
    let catalog = petramond_world::registry::load_catalog(
        texts,
        |text| {
            serde_json::from_str::<RawFile>(text).map(|file| {
                fluid_layers.push(fluids::Layer {
                    pools: file.fluid_pools,
                    falls: file.fluid_falls,
                });
                file.underground_biomes
            })
        },
        |r| &r.underground_biome,
        ENGINE_UNDERGROUND_BIOME_NAMES,
        "underground biome",
        |r, id, names| {
            let name = names.name(id).expect("id resolved from this table");
            convert(r, id as u8, name).map_err(|e| format!("underground biome '{name}': {e}"))
        },
    )?;
    Ok(compile(catalog, fluids::compile(fluid_layers)?))
}

fn convert(
    r: RawUndergroundBiome,
    id: u8,
    name: &'static str,
) -> Result<UndergroundBiomeDef, String> {
    match (r.climate, id) {
        (Some(_), 0) => {
            return Err("the ordinary fallback must not declare a climate selector".into())
        }
        (None, 0) => {}
        (None, _) => return Err("a habitat requires a climate selector".into()),
        (Some(range), _) => range.validate()?,
    }
    if let Some(region) = &r.region {
        if id == 0 {
            return Err("the ordinary fallback must not declare a region".into());
        }
        region.validate()?;
    }

    let whole_column = r.y.is_none();
    let y = match r.y {
        None => (WORLD_MIN_Y, WORLD_MAX_Y - 1),
        Some([lo, hi]) => {
            if lo > hi || lo < WORLD_MIN_Y || hi >= WORLD_MAX_Y {
                return Err(format!(
                    "'y' band [{lo}, {hi}] must be increasing and inside \
                     [{WORLD_MIN_Y}, {}]",
                    WORLD_MAX_Y - 1
                ));
            }
            (lo, hi)
        }
    };

    let (aquifer, barrier_name, aquifer_fluid_name) = if let Some(a) = r.aquifer {
        if !(y.0..=y.1).contains(&a.level) || a.level < CAVE_MIN_Y {
            return Err(
                "'aquifer.level' must lie in the carvable part of the row's depth band".into(),
            );
        }
        if !a.barrier.is_opaque()
            || !a
                .barrier
                .static_collision_boxes()
                .is_some_and(|boxes| boxes.iter().any(|b| b.min == [0.0; 3] && b.max == [1.0; 3]))
        {
            return Err("'aquifer.barrier' must be an opaque solid block".into());
        }
        if !a.fluid.is_fluid() {
            return Err("'aquifer.fluid' must name a fluid block".into());
        }
        let name = |block: Block| {
            petramond_world::registry::names()
                .blocks
                .name(block.id())
                .unwrap_or("?")
        };
        (
            Some(Aquifer {
                level: a.level,
                barrier: a.barrier.id(),
                fluid: a.fluid.id(),
            }),
            name(a.barrier),
            name(a.fluid),
        )
    } else {
        (None, "", "")
    };

    let (lining, lining_name, shell, blend, faces, face_names, pattern) = match r.lining {
        None => (0u16, "", 1.0, default_blend(), None, [""; 3], None),
        Some(l) => {
            if !(l.shell > 0.0 && l.shell <= SHELL_MAX) {
                return Err(format!(
                    "'lining.shell' {} is outside (0, {SHELL_MAX}]",
                    l.shell
                ));
            }
            // Air is the carver's "no lining" sentinel, so a row asking for it
            // would silently do nothing instead of what it says.
            if l.block.id() == Block::Air.id() {
                return Err(
                    "'lining.block' must not be air: a lining PAINTS the cave wall; \
                     omit 'lining' for bare stone"
                        .into(),
                );
            }
            let block_name = petramond_world::registry::names()
                .blocks
                .name(l.block.id())
                .unwrap_or("?");
            let (faces, face_names) = match l.faces {
                None => (None, [""; 3]),
                Some(f) => convert_faces(f, l.block.id(), name, id, aquifer)?,
            };
            (
                l.block.id(),
                block_name,
                l.shell,
                l.blend,
                faces,
                face_names,
                l.pattern.map(pattern::RawPattern::resolve).transpose()?,
            )
        }
    };

    if !(blend[0] >= 0.0 && blend[0] <= BLEND_FIELD_MAX) {
        return Err(format!(
            "'lining.blend' field width {} is outside [0, {BLEND_FIELD_MAX}]",
            blend[0]
        ));
    }
    if !(blend[1] >= 0.0 && blend[1] <= BLEND_Y_MAX) {
        return Err(format!(
            "'lining.blend' block width {} is outside [0, {BLEND_Y_MAX}]",
            blend[1]
        ));
    }

    Ok(UndergroundBiomeDef {
        name,
        climate: r.climate,
        region: r.region,
        y,
        whole_column,
        lining,
        lining_name,
        aquifer,
        barrier_name,
        aquifer_fluid_name,
        faces,
        face_names,
        shell,
        pattern,
        geology: r.geology.map(pattern::RawPattern::resolve).transpose()?,
        // An unbounded height rule has no vertical lining edge to feather.
        blend: (blend[0], if whole_column { 0.0 } else { blend[1] }),
    })
}

fn convert_faces(
    f: RawFaces,
    base: u16,
    name: &'static str,
    biome: u8,
    aquifer: Option<Aquifer>,
) -> Result<ConvertedFaces, String> {
    let floor_depth = f.floor_depth.resolve()?;
    let one = |raw: Option<RawFace>, label: &str| -> Result<FaceLining, String> {
        let Some(raw) = raw else {
            return Ok(FaceLining {
                block: base,
                weight: 1.0,
                pattern: None,
            });
        };
        if !(0.0..=1.0).contains(&raw.weight) {
            return Err(format!(
                "'lining.faces.{label}.weight' {} is outside [0, 1]",
                raw.weight
            ));
        }
        Ok(FaceLining {
            block: raw.block.map_or(base, |b| b.id()),
            weight: raw.weight,
            pattern: raw.pattern.map(pattern::RawPattern::resolve).transpose()?,
        })
    };
    let named = [f.floor.is_some(), f.wall.is_some(), f.ceiling.is_some()];
    let floor = one(f.floor, "floor")?;
    let wall = one(f.wall, "wall")?;
    let ceiling = one(f.ceiling, "ceiling")?;
    let names = std::array::from_fn(|i| {
        if named[i] {
            petramond_world::registry::names()
                .blocks
                .name([floor, wall, ceiling][i].block)
                .unwrap_or("?")
        } else {
            ""
        }
    });
    if f.floor_under.is_some() && floor_depth.max() < 2 {
        return Err(
            "'lining.faces.floor_under' needs 'floor_depth' of at least 2, or it paints nothing"
                .into(),
        );
    }
    let floor_under = f
        .floor_under
        .map(|raw| one(Some(raw), "floor_under"))
        .transpose()?;
    let floor_submerged = f
        .floor_submerged
        .map(|raw| one(Some(raw), "floor_submerged"))
        .transpose()?;
    let submerged_in: Vec<u16> = match (f.submerged_in, aquifer) {
        (Some(fluids), _) => fluids.iter().map(|b| b.id()).collect(),
        (None, Some(aquifer)) => vec![aquifer.fluid],
        (None, None) => Vec::new(),
    };
    if floor_submerged.is_some() == submerged_in.is_empty() {
        return Err(if submerged_in.is_empty() {
            "'lining.faces.floor_submerged' needs an 'aquifer' or 'submerged_in', or it paints nothing"
        } else {
            "'lining.faces.submerged_in' needs a 'floor_submerged' to apply"
        }
        .into());
    }
    if submerged_in
        .iter()
        .any(|&id| !Block::from_id(id).is_fluid())
    {
        return Err("'lining.faces.submerged_in' must name fluid blocks".into());
    }
    Ok((
        Some(LiningFaces {
            biome,
            floor,
            wall,
            ceiling,
            floor_depth,
            floor_under,
            floor_submerged,
            submerged_in: Box::leak(submerged_in.into_boxed_slice()),
            salt: fnv64(b"lining:").wrapping_mul(FNV_PRIME) ^ fnv64(name.as_bytes()),
        }),
        names,
    ))
}

fn compile(catalog: Catalog<UndergroundBiomeDef>, fluids: fluids::Rows) -> UndergroundBiomes {
    let rows = catalog.rows();
    let base = rows[0].shell;

    let mut selectors: Vec<u8> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.climate.is_some() && r.region.is_none())
        .map(|(i, _)| i as u8)
        .collect();
    selectors.sort_by_key(|&id| rows[id as usize].name);

    let base_bounds = base.max(1.0);
    let bounds = rows
        .iter()
        .fold(base_bounds, |bound, row| bound.max(row.shell));

    let mut lining = [0u16; 256];
    let mut faces: Box<[Option<LiningFaces>; 256]> = Box::new([None; 256]);
    let mut patterns = Box::new([None; 256]);
    let mut geology = Box::new([None; 256]);
    let mut geology_ids = IdSet::default();
    for (id, r) in rows.iter().enumerate() {
        lining[id] = r.lining;
        faces[id] = r.faces;
        patterns[id] = r.pattern;
        geology[id] = r.geology;
        if r.geology.is_some() {
            geology_ids.insert(id as u8);
        }
    }
    let mut aquifers = [None; 256];
    let mut aquifer_y_span: Option<(i32, i32)> = None;
    for (id, row) in rows.iter().enumerate() {
        if let Some(aquifer) = row.aquifer {
            aquifers[id] = Some(aquifer);
            let span = (row.y.0.max(CAVE_MIN_Y), aquifer.level);
            aquifer_y_span =
                Some(aquifer_y_span.map_or(span, |old| (old.0.min(span.0), old.1.max(span.1))));
        }
    }
    let lining_faces_vary = faces.iter().any(|f| f.is_some());
    let lining_floor_under_world_floor = rows
        .iter()
        .any(|r| r.faces.is_some_and(|f| f.floor.block != 0) && r.y.0 < CAVE_MIN_Y);
    let lining_floor_depth_max = rows
        .iter()
        .filter_map(|r| r.faces.filter(|f| f.floor.block != 0))
        .map(|f| f.floor_depth.max())
        .max()
        .unwrap_or(0);

    let fingerprint = fingerprint(rows, &fluids);
    let regions = regions::compile(rows);
    let geology_via_climate = selectors.iter().any(|&id| geology_ids.contains(id));
    UndergroundBiomes {
        catalog,
        selectors: selectors.into_boxed_slice(),
        regions,
        base,
        lining,
        patterns,
        geology,
        geology_ids,
        geology_via_climate,
        aquifers,
        aquifer_y_span,
        faces,
        lining_faces_vary,
        lining_floor_under_world_floor,
        lining_floor_depth_max,
        bounds,
        pools: fluids.pools,
        falls: fluids.falls,
        fingerprint,
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

const FNV_PRIME: u64 = 0x1_0000_0000_01b3;

pub(super) fn fnv64(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(FNV_OFFSET, |h, b| (h ^ *b as u64).wrapping_mul(FNV_PRIME))
}

/// FNV-1a over the compiled table's canonical form. Identity for the column-gen
/// cache: two runs whose tables hash alike generate identical columns.
///
/// EVERY field the carver reads has to be here. This is a hand-written list, so
/// adding a knob without adding it here serves stale `top_surf` columns from
/// the cache with no version byte moving — a retune that silently half-applies.
fn fingerprint(rows: &[UndergroundBiomeDef], fluids: &fluids::Rows) -> u64 {
    let mut h = FNV_OFFSET;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ *b as u64).wrapping_mul(FNV_PRIME);
        }
    };
    for r in rows {
        eat(r.name.as_bytes());
        eat(&serde_json::to_vec(&r.region).expect("validated region"));
        eat(r.lining_name.as_bytes());
        if let Some(pattern) = r.pattern {
            eat(&pattern.fingerprint.to_le_bytes());
        }
        eat(&[u8::from(r.geology.is_some())]);
        if let Some(pattern) = r.geology {
            eat(&pattern.fingerprint.to_le_bytes());
        }
        eat(&[u8::from(r.climate.is_some())]);
        if let Some(climate) = r.climate {
            for range in climate.axes() {
                for value in range {
                    eat(&value.to_le_bytes());
                }
            }
        }
        eat(&[u8::from(r.aquifer.is_some())]);
        if let Some(aquifer) = r.aquifer {
            eat(&aquifer.level.to_le_bytes());
            eat(r.barrier_name.as_bytes());
            eat(r.aquifer_fluid_name.as_bytes());
        }
        for v in [r.shell, r.blend.0, r.blend.1] {
            eat(&v.to_bits().to_le_bytes());
        }
        eat(&r.y.0.to_le_bytes());
        eat(&r.y.1.to_le_bytes());
        eat(&[u8::from(r.whole_column)]);
        eat(&[u8::from(r.faces.is_some())]);
        if let Some(f) = r.faces {
            for n in r.face_names {
                eat(n.as_bytes());
            }
            for face in [f.floor, f.wall, f.ceiling] {
                eat(&face.weight.to_bits().to_le_bytes());
                eat(&face.pattern.map_or(0, |p| p.fingerprint).to_le_bytes());
            }
            eat(&f.floor_depth.max().to_le_bytes());
            eat(&f.floor_depth.fingerprint().to_le_bytes());
            eat(&f.salt.to_le_bytes());
            eat(&[
                u8::from(f.floor_under.is_some()),
                u8::from(f.floor_submerged.is_some()),
            ]);
            for under in [f.floor_under, f.floor_submerged].into_iter().flatten() {
                eat(petramond_world::registry::names()
                    .blocks
                    .name(under.block)
                    .expect("resolved lining")
                    .as_bytes());
                eat(&under.weight.to_le_bytes());
                eat(&under.pattern.map_or(0, |p| p.fingerprint).to_le_bytes());
            }
            for &fluid in f.submerged_in {
                eat(petramond_world::registry::names()
                    .blocks
                    .name(fluid)
                    .expect("resolved fluid")
                    .as_bytes());
            }
        }
    }
    fluids::fingerprint(fluids, &mut eat);
    h
}
