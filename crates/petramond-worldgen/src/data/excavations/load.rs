use super::*;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    excavations: Vec<RawExcavation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExcavation {
    excavation: String,
    placement: RawPlacement,
    #[serde(default)]
    chamber: Option<RawChamber>,
    #[serde(default)]
    field: Option<RawFieldShape>,
    #[serde(default)]
    connections: Option<Connections>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlacement {
    spacing: i32,
    #[serde(default = "one_i32")]
    one_in: i32,
    y: [i32; 2],
    #[serde(default)]
    underground_biome: Option<String>,
    #[serde(default)]
    contact: Option<Contact>,
}

pub(super) fn parse_layers(
    texts: &[&str],
    biomes: &UndergroundBiomes,
) -> Result<Excavations, String> {
    let catalog = petramond_world::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.excavations),
        |r| &r.excavation,
        &[],
        "excavation",
        |r, id, names| {
            let name = names.name(id).expect("registered excavation");
            let [lo, hi] = r.placement.y;
            if lo > hi || lo < WORLD_MIN_Y || hi >= WORLD_MAX_Y {
                return Err(format!("excavation '{name}': invalid placement depth"));
            }
            let underground_biome = r
                .placement
                .underground_biome
                .as_ref()
                .map(|key| {
                    biomes.id(key).ok_or_else(|| {
                        format!("excavation '{name}': unknown underground biome '{key}'")
                    })
                })
                .transpose()?;
            let placement = Placement {
                spacing: r.placement.spacing,
                one_in: r.placement.one_in,
                y: (lo, hi),
                underground_biome,
                contact: r.placement.contact,
            };
            if placement.spacing < 16 || placement.spacing > 1024 || placement.one_in < 1 {
                return Err(format!(
                    "excavation '{name}': invalid placement spacing or chance"
                ));
            }
            let shape = match (r.chamber, r.field) {
                (Some(raw), None) => {
                    let chamber = convert_chamber(raw, &placement)
                        .map_err(|e| format!("excavation '{name}': {e}"))?;
                    if let Some(c) = &r.connections {
                        validate_connections(c, &placement, &chamber)
                            .map_err(|e| format!("excavation '{name}': {e}"))?;
                    }
                    ExcavationShape::Chamber(chamber)
                }
                (None, Some(raw)) if r.connections.is_none() && placement.contact.is_none() => {
                    ExcavationShape::Field(Box::new(raw.resolve(placement.spacing)
                        .map_err(|e| format!("excavation '{name}': {e}"))?))
                }
                _ => return Err(format!("excavation '{name}': select one shape; field shapes cannot declare chamber connections or contact")),
            };
            Ok(Excavation {
                name,
                salt: hash(name.as_bytes()),
                placement,
                shape,
                connections: r.connections,
            })
        },
    )?;
    let mut rows: Vec<_> = catalog.rows().iter().collect();
    rows.sort_by_key(|r| r.name);
    let y_span = rows.iter().fold(None, |span: Option<(i32, i32)>, row| {
        let band = row.y_span();
        Some(span.map_or(band, |(lo, hi)| (lo.min(band.0), hi.max(band.1))))
    });
    let mut bytes = Vec::new();
    for row in &rows {
        bytes.extend_from_slice(row.name.as_bytes());
        bytes.push(0);
        if let Some(id) = row.placement.underground_biome {
            bytes.extend_from_slice(biomes.name(id).expect("resolved biome").as_bytes());
        }
        bytes.push(match row.placement.contact {
            None => 0,
            Some(Contact::NaturalCave) => 1,
        });
        for value in [
            row.placement.spacing,
            row.placement.one_in,
            row.placement.y.0,
            row.placement.y.1,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let Some(c) = row.chamber() else {
            bytes.push(1);
            bytes.extend_from_slice(&row.field().expect("field shape").fingerprint.to_le_bytes());
            continue;
        };
        bytes.push(0);
        for value in [c.r_min, c.r_max, c.lobes] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [
            c.flatten,
            c.stretch.0,
            c.stretch.1,
            c.sill,
            c.feather,
            c.strength,
            c.lobe_spread,
            c.lobe_scale.0,
            c.lobe_scale.1,
            c.tunnel,
            c.rim_noise,
        ] {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        bytes.push(u8::from(row.connections.is_some()));
        if let Some(c) = row.connections {
            for value in [c.radius[0], c.radius[1], c.flatten, c.feather, c.bend] {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
        }
    }
    let fields: Vec<_> = rows.iter().filter_map(|r| r.field()).collect();
    let field_y_span = fields.iter().fold(None, |span: Option<(i32, i32)>, f| {
        Some(span.map_or((f.y[0], f.y[1]), |(lo, hi)| {
            (lo.min(f.y[0]), hi.max(f.y[1]))
        }))
    });
    let surface_offset = fields.iter().map(|f| f.surface_offset).max().unwrap_or(0);
    Ok(Excavations {
        field_y_span,
        surface_offset,
        rows,
        y_span,
        fingerprint: hash(&bytes),
    })
}

fn validate_connections(
    c: &Connections,
    placement: &Placement,
    chamber: &Chamber,
) -> Result<(), String> {
    if !(c.radius[0] >= 2.0 && c.radius[0] <= c.radius[1] && c.radius[1] <= 12.0)
        || !(0.5..=1.5).contains(&c.flatten)
        || !(CHAMBER_FEATHER_MIN..=24.0).contains(&c.feather)
        || !(0.0..=0.35).contains(&c.bend)
    {
        return Err("invalid connections: radius must be ordered in [2,12], flatten in [0.5,1.5], feather in [8,24], bend in [0,0.35]".into());
    }
    let (drop, rise) = chamber.extent_y();
    let reach = c.reach_y();
    let band = Chamber::placement_band(placement.y);
    let first = (band.0 + drop.max(reach) + CAVE_LATTICE_STEP - 1).div_euclid(CAVE_LATTICE_STEP);
    let last = (band.1 - rise.max(reach)).div_euclid(CAVE_LATTICE_STEP);
    if first > last {
        return Err("placement depth cannot contain connected rooms and their passage rims".into());
    }
    Ok(())
}

pub(super) fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, &byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}

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

fn convert_chamber(c: RawChamber, placement: &Placement) -> Result<Chamber, String> {
    let y = placement.y;
    let (lo, hi) = CHAMBER_LATTICE_RANGE;
    if placement.spacing < lo
        || placement.spacing > hi
        || placement.spacing % CAVE_LATTICE_STEP != 0
    {
        return Err(format!(
            "'placement.spacing' {} must be in [{lo}, {hi}] and a multiple of {CAVE_LATTICE_STEP}",
            placement.spacing
        ));
    }
    if placement.one_in < 1 || placement.one_in > CHAMBER_ONE_IN_MAX {
        return Err(format!(
            "'chamber.one_in' {} is outside [1, {CHAMBER_ONE_IN_MAX}]",
            placement.one_in
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
    let [stretch_min, stretch_max] = c.stretch;
    if !(1.0..=4.0).contains(&stretch_min) || !(stretch_min..=4.0).contains(&stretch_max) {
        return Err("'chamber.stretch' must be increasing and inside [1, 4]".into());
    }
    let chamber = Chamber {
        r_min,
        r_max,
        flatten: c.flatten,
        stretch: (stretch_min, stretch_max),
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
    if 2 * reach > placement.spacing {
        return Err(format!(
            "'chamber' reaches {reach} blocks from its centre, more than half its \
             'lattice' {}: raise the lattice or shrink radius/feather",
            placement.spacing
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
