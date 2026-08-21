//! The underground-biome catalog LOADER: the raw JSON file shape, the value
//! ranges every field is validated against, and the conversion into the
//! resolved table the queries read.
//!
//! Kept apart from the vocabulary it produces, the way `block/load.rs` is kept
//! apart from `block.rs`.

use super::*;

#[derive(Deserialize)]
struct RawFile {
    underground_biomes: Vec<RawUndergroundBiome>,
}

const TUNNEL_RANGE: (f64, f64) = (0.25, 4.0);

const CHEESE_RANGE: (f64, f64) = (-0.5, 0.5);

/// A `shell` past ~2 stops reading as a LINING and starts painting most of the
/// biome's rock (measured: 4.0 lines over half the sampled volume), and it
/// widens the skip mask for the whole world, so it costs every generating
/// thread. Legal, deliberately — some biome may want cathedral-thick walls —
/// but it is a heavy choice, not a free one.
const SHELL_MAX: f64 = 8.0;

const BLEND_FIELD_MAX: f64 = 0.5;

const BLEND_Y_MAX: f64 = 64.0;

const CHAMBER_LATTICE_RANGE: (i32, i32) = (CAVE_LATTICE_STEP * 4, 1024);

const CHAMBER_ONE_IN_MAX: i32 = 4096;

const CHAMBER_RADIUS_RANGE: (i32, i32) = (4, 64);

const CHAMBER_FLATTEN_RANGE: (f64, f64) = (0.1, 2.0);

const CHAMBER_FEATHER_MAX: f64 = 48.0;

/// A term of 1.0 already lifts the cavern threshold clear of the sampler's
/// whole range, i.e. unconditionally open. Past ~2 an author is only making
/// the skip mask give up over a wider rim for no visible gain.
const CHAMBER_STRENGTH_MAX: f64 = 2.0;

/// A chamber is trilinear on the cave lattice, so a rim that ramps faster than
/// two lattice steps comes out visibly faceted — octahedral, which is the one
/// artefact that reads as an engine bug rather than as rock. The rim is
/// measured in BLOCKS in every direction, so this bound now means what it says
/// vertically too (it used to be scaled by `flatten` there and was routinely
/// violated in silence).
const CHAMBER_FEATHER_MIN: f64 = 2.0 * CAVE_LATTICE_STEP as f64;

const CHAMBER_LOBES_MAX: i32 = 4;

const CHAMBER_LOBE_SPREAD_MAX: f64 = 1.0;

const CHAMBER_LOBE_SCALE_MAX: f64 = 1.5;

/// A room already lifts the cavern threshold clear of the sampler; past this a
/// tunnel gain only makes the skip mask give up over a wider rim.
const CHAMBER_TUNNEL_GAIN_MAX: f64 = 4.0;

/// The kneading field reaches about ±0.54, so past this the ramp can invert and
/// the rim collapses onto the analytic core in patches — the artefact the knob
/// exists to remove.
const CHAMBER_RIM_NOISE_MAX: f64 = 1.8;

/// Deeper than this and the floor rule's cells can escape the one lattice cell
/// the skip mask is dilated by.
const LINING_FLOOR_DEPTH_MAX: i32 = CAVE_LATTICE_STEP;

pub(super) fn parse_layers(texts: &[&str]) -> Result<UndergroundBiomes, String> {
    let catalog = petramond_world::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.underground_biomes),
        |r| &r.underground_biome,
        ENGINE_UNDERGROUND_BIOME_NAMES,
        "underground biome",
        |r, id, names| {
            let name = names.name(id).expect("id resolved from this table");
            convert(r, id as u8, name).map_err(|e| format!("underground biome '{name}': {e}"))
        },
    )?;
    Ok(compile(catalog))
}

fn convert(
    r: RawUndergroundBiome,
    id: u8,
    name: &'static str,
) -> Result<UndergroundBiomeDef, String> {
    let band = match (r.field, id) {
        // The fallback slot is structural: a band on it could never be
        // reached (`id_at` answers 0 only when nothing matched anyway).
        (Some(_), 0) => {
            return Err(
                "must not declare a 'field' band: the fallback row owns every \
                        cell no other row claims"
                    .into(),
            )
        }
        // A banded-less pack row can never generate; that is a typo, not a
        // design.
        (None, 0) => None,
        (None, _) => return Err("no 'field' band, so it could never generate".into()),
        (Some([lo, hi]), _) => {
            if !lo.is_finite() || !hi.is_finite() || lo >= hi {
                return Err(format!(
                    "'field' band [{lo}, {hi}] must be finite and increasing"
                ));
            }
            // Half-open, so the band claims a cell iff some REALIZABLE field
            // value `v` satisfies `lo <= v < hi`. A band that misses the
            // field's reachable interval entirely loads fine and then never
            // generates a thing, which is the same authoring mistake as a row
            // with no band at all — and just as silent.
            let reach = CAVE_BIOME_FIELD_MAX;
            if !(lo <= reach && hi > -reach) {
                return Err(format!(
                    "'field' band [{lo}, {hi}) never meets the cave-biome field's \
                     reachable range (±{reach}), so it could never generate"
                ));
            }
            Some((lo, hi))
        }
    };

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

    let (lining, lining_name, shell, faces, face_names) = match r.lining {
        None => (0u16, "", 1.0, None, [""; 3]),
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
                Some(f) => convert_faces(f, l.block.id(), name)?,
            };
            (l.block.id(), block_name, l.shell, faces, face_names)
        }
    };

    let c = r.caliber.unwrap_or(RawCaliber {
        tunnel: 1.0,
        cheese: 0.0,
        blend: default_blend(),
    });
    if !(c.tunnel >= TUNNEL_RANGE.0 && c.tunnel <= TUNNEL_RANGE.1) {
        return Err(format!(
            "'caliber.tunnel' {} is outside [{}, {}]",
            c.tunnel, TUNNEL_RANGE.0, TUNNEL_RANGE.1
        ));
    }
    if !(c.cheese >= CHEESE_RANGE.0 && c.cheese <= CHEESE_RANGE.1) {
        return Err(format!(
            "'caliber.cheese' {} is outside [{}, {}]",
            c.cheese, CHEESE_RANGE.0, CHEESE_RANGE.1
        ));
    }
    if !(c.blend[0] >= 0.0 && c.blend[0] <= BLEND_FIELD_MAX) {
        return Err(format!(
            "'caliber.blend' field width {} is outside [0, {BLEND_FIELD_MAX}]",
            c.blend[0]
        ));
    }
    if !(c.blend[1] >= 0.0 && c.blend[1] <= BLEND_Y_MAX) {
        return Err(format!(
            "'caliber.blend' block width {} is outside [0, {BLEND_Y_MAX}]",
            c.blend[1]
        ));
    }

    let chamber = r
        .chamber
        .map(|c| convert_chamber(c, id, name, y, !whole_column))
        .transpose()?;

    Ok(UndergroundBiomeDef {
        name,
        band,
        y,
        lining,
        lining_name,
        faces,
        face_names,
        chamber,
        caliber: Caliber {
            tunnel: c.tunnel,
            cheese: c.cheese,
            shell,
        },
        // A row claiming the whole column has no depth rim, so nothing to
        // feather across: "absent 'y'" must not quietly taper the caliber
        // towards the world floor and ceiling.
        blend: (c.blend[0], if whole_column { 0.0 } else { c.blend[1] }),
    })
}

fn convert_faces(f: RawFaces, base: u16, name: &'static str) -> Result<ConvertedFaces, String> {
    if !(1..=LINING_FLOOR_DEPTH_MAX).contains(&f.floor_depth) {
        return Err(format!(
            "'lining.faces.floor_depth' {} is outside [1, {LINING_FLOOR_DEPTH_MAX}]",
            f.floor_depth
        ));
    }
    let mut names = [""; 3];
    let mut one = |raw: Option<RawFace>, which: usize, label: &str| -> Result<FaceLining, String> {
        let Some(raw) = raw else {
            // An orientation the row does not mention keeps the row's own
            // lining at full coverage, so naming one face is not a silent
            // opt-out of the other two.
            names[which] = "";
            return Ok(FaceLining {
                block: base,
                weight: 1.0,
            });
        };
        if !(0.0..=1.0).contains(&raw.weight) {
            return Err(format!(
                "'lining.faces.{label}.weight' {} is outside [0, 1]",
                raw.weight
            ));
        }
        let block = raw.block.map_or(base, |b| b.id());
        // Unlike `lining.block`, air here is meaningful: "line the walls but
        // leave the ceiling bare" is a real thing to want.
        names[which] = petramond_world::registry::names()
            .blocks
            .name(block)
            .unwrap_or("?");
        Ok(FaceLining {
            block,
            weight: raw.weight,
        })
    };
    let floor = one(f.floor, 0, "floor")?;
    let wall = one(f.wall, 1, "wall")?;
    let ceiling = one(f.ceiling, 2, "ceiling")?;
    // A subsurface with nothing above it is a typo, not a layering: the clause
    // only ever paints cells BELOW the course top.
    if f.floor_under.is_some() && f.floor_depth < 2 {
        return Err(
            "'lining.faces.floor_under' needs 'floor_depth' of at least 2, or it paints nothing"
                .to_string(),
        );
    }
    let floor_under = match f.floor_under {
        Some(raw) => {
            if !(0.0..=1.0).contains(&raw.weight) {
                return Err(format!(
                    "'lining.faces.floor_under.weight' {} is outside [0, 1]",
                    raw.weight
                ));
            }
            Some(FaceLining {
                block: raw.block.map_or(base, |b| b.id()),
                weight: raw.weight,
            })
        }
        None => None,
    };
    Ok((
        Some(LiningFaces {
            floor,
            wall,
            ceiling,
            floor_depth: f.floor_depth,
            floor_under,
            salt: fnv64(b"lining:").wrapping_mul(FNV_PRIME) ^ fnv64(name.as_bytes()),
        }),
        names,
    ))
}

fn convert_chamber(
    c: RawChamber,
    id: u8,
    name: &'static str,
    y: (i32, i32),
    declared_y: bool,
) -> Result<Chamber, String> {
    // The fallback row owns every cell no other row claims, so a chamber on it
    // would roll across the entire world with no territory to confine it.
    if id == 0 {
        return Err("must not declare a 'chamber': the fallback row has no territory".into());
    }
    // A room is placed INSIDE its row's depth band, and the carver arms its
    // chamber lane over exactly that band. Without one the lane would be armed
    // for every section in the world to place rooms in open sky, where the
    // surface buffer makes them do nothing at all.
    if !declared_y {
        return Err("'chamber' needs a 'y' band: it is the depth the room is placed in".into());
    }
    let (lo, hi) = CHAMBER_LATTICE_RANGE;
    if c.lattice < lo || c.lattice > hi || c.lattice % CAVE_LATTICE_STEP != 0 {
        return Err(format!(
            "'chamber.lattice' {} must be in [{lo}, {hi}] and a multiple of {CAVE_LATTICE_STEP}",
            c.lattice
        ));
    }
    if c.one_in < 1 || c.one_in > CHAMBER_ONE_IN_MAX {
        return Err(format!(
            "'chamber.one_in' {} is outside [1, {CHAMBER_ONE_IN_MAX}]",
            c.one_in
        ));
    }
    let [r_min, r_max] = c.radius;
    let (rlo, rhi) = CHAMBER_RADIUS_RANGE;
    if r_min > r_max || r_min < rlo || r_max > rhi {
        return Err(format!(
            "'chamber.radius' [{r_min}, {r_max}] must be increasing and inside [{rlo}, {rhi}]"
        ));
    }
    if !(c.flatten >= CHAMBER_FLATTEN_RANGE.0 && c.flatten <= CHAMBER_FLATTEN_RANGE.1) {
        return Err(format!(
            "'chamber.flatten' {} is outside [{}, {}]",
            c.flatten, CHAMBER_FLATTEN_RANGE.0, CHAMBER_FLATTEN_RANGE.1
        ));
    }
    if !(c.sill >= 0.0 && c.sill <= 1.0) {
        return Err(format!("'chamber.sill' {} is outside [0, 1]", c.sill));
    }
    if !(c.feather >= CHAMBER_FEATHER_MIN && c.feather <= CHAMBER_FEATHER_MAX) {
        return Err(format!(
            "'chamber.feather' {} is outside [{CHAMBER_FEATHER_MIN}, {CHAMBER_FEATHER_MAX}]; \
             a rim narrower than two lattice steps comes out faceted",
            c.feather
        ));
    }
    if !(c.strength > 0.0 && c.strength <= CHAMBER_STRENGTH_MAX) {
        return Err(format!(
            "'chamber.strength' {} is outside (0, {CHAMBER_STRENGTH_MAX}]",
            c.strength
        ));
    }
    if !(1..=CHAMBER_LOBES_MAX).contains(&c.lobes) {
        return Err(format!(
            "'chamber.lobes' {} is outside [1, {CHAMBER_LOBES_MAX}]",
            c.lobes
        ));
    }
    if !(c.lobe_spread >= 0.0 && c.lobe_spread <= CHAMBER_LOBE_SPREAD_MAX) {
        return Err(format!(
            "'chamber.lobe_spread' {} is outside [0, {CHAMBER_LOBE_SPREAD_MAX}]",
            c.lobe_spread
        ));
    }
    let [s_min, s_max] = c.lobe_scale;
    if !(s_min > 0.0 && s_min <= s_max && s_max <= CHAMBER_LOBE_SCALE_MAX) {
        return Err(format!(
            "'chamber.lobe_scale' [{s_min}, {s_max}] must be increasing and inside \
             (0, {CHAMBER_LOBE_SCALE_MAX}]"
        ));
    }
    // A satellite offset the full spread on all three axes sits
    // `spread * sqrt(3)` primary-radii from the centre, in the normalised space
    // where both lobes are spheres. Past `1 + smallest satellite` it can clear
    // the primary entirely and the room comes out as a big void plus a detached
    // bubble — a sealed pocket no player can reach, which is the exact failure
    // the whole attachment work exists to remove.
    if c.lobes > 1 && c.lobe_spread * 3.0f64.sqrt() >= 1.0 + s_min {
        return Err(format!(
            "'chamber.lobe_spread' {} can detach a satellite lobe of scale {s_min} from the \
             room, leaving a sealed pocket: keep spread * sqrt(3) < 1 + lobe_scale[0]",
            c.lobe_spread
        ));
    }
    if !(c.tunnel >= 0.0 && c.tunnel <= CHAMBER_TUNNEL_GAIN_MAX) {
        return Err(format!(
            "'chamber.tunnel' {} is outside [0, {CHAMBER_TUNNEL_GAIN_MAX}]",
            c.tunnel
        ));
    }
    if !(c.rim_noise >= 0.0 && c.rim_noise <= CHAMBER_RIM_NOISE_MAX) {
        return Err(format!(
            "'chamber.rim_noise' {} is outside [0, {CHAMBER_RIM_NOISE_MAX}]",
            c.rim_noise
        ));
    }
    let chamber = Chamber {
        row: id,
        salt: fnv64(name.as_bytes()),
        lattice: c.lattice,
        one_in: c.one_in,
        r_min,
        r_max,
        flatten: c.flatten,
        sill: c.sill,
        feather: c.feather,
        strength: c.strength,
        lobes: c.lobes,
        lobe_spread: c.lobe_spread,
        lobe_scale: (s_min, s_max),
        tunnel: c.tunnel,
        rim_noise: c.rim_noise,
    };
    // One room may span at most two candidate columns per axis. That is what
    // keeps the carver's per-box candidate window a small constant instead of
    // growing with the radius — the whole performance argument for evaluating
    // this inside the lattice build.
    let reach = chamber.reach_xz();
    if 2 * reach > chamber.lattice {
        return Err(format!(
            "'chamber' reaches {reach} blocks from its centre, more than half its \
             'lattice' {}: raise the lattice or shrink radius/feather",
            chamber.lattice
        ));
    }
    // The band has to hold the biggest room the row can roll, with a lattice
    // step of slack at each end so at least one legal centre exists — measured
    // against the CARVABLE part of the band, because that is where centres are
    // actually rolled. A row whose usable window is too short is a load error
    // rather than a row that quietly generates half-height rooms sliced by the
    // world floor.
    let (drop, rise) = chamber.extent_y();
    let needed = drop + rise + 2 * CAVE_LATTICE_STEP;
    let band = Chamber::placement_band(y);
    if band.1 - band.0 < needed {
        return Err(format!(
            "'y' band [{}, {}] leaves {} carvable blocks (nothing is cut below \
             {CAVE_MIN_Y}), too short for a 'chamber' needing {needed}",
            y.0,
            y.1,
            band.1 - band.0
        ));
    }
    Ok(chamber)
}

fn compile(catalog: Catalog<UndergroundBiomeDef>) -> UndergroundBiomes {
    let rows = catalog.rows();
    let base = rows[0].caliber;

    let mut bands: Vec<Band> = rows
        .iter()
        .enumerate()
        .filter_map(|(id, r)| {
            r.band.map(|(lo, hi)| Band {
                lo,
                hi,
                y_min: r.y.0,
                y_max: r.y.1,
                id: id as u8,
                caliber: r.caliber,
                blend_f: r.blend.0,
                blend_y: r.blend.1,
            })
        })
        .collect();
    // Specificity, never load order: narrower field band first, then narrower
    // depth band, then the row's own namespaced NAME. Every key is data the row
    // declares, so enabling/disabling/reordering any pack leaves the partition
    // untouched.
    bands.sort_by(|a, b| {
        let width = |x: &Band| x.hi - x.lo;
        let span = |x: &Band| (x.y_max - x.y_min) as i64;
        width(a)
            .total_cmp(&width(b))
            .then(span(a).cmp(&span(b)))
            .then(rows[a.id as usize].name.cmp(rows[b.id as usize].name))
    });

    let caliber_varies = bands.iter().any(|b| b.caliber != base);

    // The entrance carver always uses the UNSCALED shell width, so the shell
    // bound can never drop below 1.0 even if every row shrinks its lining.
    let base_bounds = CaliberBounds {
        tunnel: base.tunnel,
        cheese: base.cheese,
        shell: base.shell.max(1.0),
    };
    let mut bounds = base_bounds;
    for r in rows {
        bounds.tunnel = bounds.tunnel.max(r.caliber.tunnel);
        bounds.cheese = bounds.cheese.max(r.caliber.cheese);
        bounds.shell = bounds.shell.max(r.caliber.shell);
    }

    let mut lining = [0u16; 256];
    let mut faces: Box<[Option<LiningFaces>; 256]> = Box::new([None; 256]);
    for (id, r) in rows.iter().enumerate() {
        lining[id] = r.lining;
        faces[id] = r.faces;
    }
    let lining_faces_vary = faces.iter().any(|f| f.is_some());
    let lining_floor_under_world_floor = rows
        .iter()
        .any(|r| r.faces.is_some_and(|f| f.floor.block != 0) && r.y.0 < CAVE_MIN_Y);
    let lining_floor_depth_max = rows
        .iter()
        .filter_map(|r| r.faces.filter(|f| f.floor.block != 0))
        .map(|f| f.floor_depth)
        .max()
        .unwrap_or(0);

    // Frozen accumulation order: row id, which the catalog assigns by layer and
    // first appearance. Never `bands` order, which sorts by specificity and so
    // reshuffles when a pack narrows a band.
    let chambers: Vec<Chamber> = rows.iter().filter_map(|r| r.chamber).collect();
    // A room is placed entirely inside its row's band, so the band IS the span
    // outside which the term is provably zero — no reach padding needed.
    let chamber_y_span = chambers.iter().fold(None, |acc: Option<(i32, i32)>, c| {
        let span = rows[c.row as usize].y;
        Some(acc.map_or(span, |a| (a.0.min(span.0), a.1.max(span.1))))
    });

    let (cuts, segments, candidates) = index_field_axis(&bands);
    let fingerprint = fingerprint(rows);
    let table = UndergroundBiomes {
        catalog,
        caliber_varies,
        bands: bands.into_boxed_slice(),
        cuts,
        segments,
        candidates,
        base,
        lining,
        faces,
        lining_faces_vary,
        lining_floor_under_world_floor,
        lining_floor_depth_max,
        bounds,
        base_bounds,
        fingerprint,
        chambers: chambers.into_boxed_slice(),
        chamber_y_span,
    };
    // A pack author's first question when a biome "does not generate" is which
    // bands exist and who won the overlap. The compiled order is the answer,
    // and stating it once at load beats guessing from carved output.
    for b in table.bands.iter() {
        log::info!(
            "underground biome '{}': field [{}, {}), y [{}, {}]",
            table.name(b.id).unwrap_or("?"),
            b.lo,
            b.hi,
            b.y_min,
            b.y_max
        );
    }
    table
}

fn index_field_axis(bands: &[Band]) -> FieldAxisIndex {
    let mut cuts: Vec<f64> = bands.iter().flat_map(|b| [b.lo, b.hi]).collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();

    let mut segments = Vec::with_capacity(cuts.len() + 1);
    let mut candidates: Vec<u8> = Vec::new();
    for k in 0..=cuts.len() {
        let start = candidates.len() as u32;
        // Segment k spans `[cuts[k-1], cuts[k])`, so its LOW endpoint lies
        // inside it and is itself a cut: a band whose half-open interval
        // contains that one point contains the whole segment. Segments 0 and
        // `cuts.len()` reach past every endpoint and stay empty.
        if let Some(&probe) = k.checked_sub(1).and_then(|i| cuts.get(i)) {
            candidates.extend(
                bands
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| probe >= b.lo && probe < b.hi)
                    .map(|(i, _)| i as u8),
            );
        }
        segments.push((start, candidates.len() as u32));
    }
    (
        cuts.into_boxed_slice(),
        segments.into_boxed_slice(),
        candidates.into_boxed_slice(),
    )
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

const FNV_PRIME: u64 = 0x1_0000_0000_01b3;

fn fnv64(bytes: &[u8]) -> u64 {
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
fn fingerprint(rows: &[UndergroundBiomeDef]) -> u64 {
    let mut h = FNV_OFFSET;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ *b as u64).wrapping_mul(FNV_PRIME);
        }
    };
    for r in rows {
        eat(r.name.as_bytes());
        eat(r.lining_name.as_bytes());
        let (lo, hi) = r.band.unwrap_or((f64::NAN, f64::NAN));
        for v in [
            lo,
            hi,
            r.caliber.tunnel,
            r.caliber.cheese,
            r.caliber.shell,
            r.blend.0,
            r.blend.1,
        ] {
            eat(&v.to_bits().to_le_bytes());
        }
        eat(&r.y.0.to_le_bytes());
        eat(&r.y.1.to_le_bytes());
        if let Some(f) = r.faces {
            for n in r.face_names {
                eat(n.as_bytes());
            }
            for face in [f.floor, f.wall, f.ceiling] {
                eat(&face.weight.to_bits().to_le_bytes());
            }
            eat(&f.floor_depth.to_le_bytes());
            eat(&f.salt.to_le_bytes());
        }
        if let Some(c) = r.chamber {
            for v in [
                c.flatten,
                c.sill,
                c.feather,
                c.strength,
                c.lobe_spread,
                c.lobe_scale.0,
                c.lobe_scale.1,
                c.tunnel,
                c.rim_noise,
            ] {
                eat(&v.to_bits().to_le_bytes());
            }
            for v in [c.lattice, c.one_in, c.r_min, c.r_max, c.lobes] {
                eat(&v.to_le_bytes());
            }
            eat(&c.salt.to_le_bytes());
        }
    }
    h
}
