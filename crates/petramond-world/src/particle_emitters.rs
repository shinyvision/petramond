//! Keyed particle-emitter bundles: the layered `particle_emitters.json`
//! catalog (a catalog like `effects.json` — see [`crate::effect`] for the
//! pattern).
//!
//! A BUNDLE is one named visual effect: one or more particle rows (the shared
//! [`ParticleEmitter`] schema blocks use) plus an
//! optional body tint and self-lighting. Engine bundles own the low ids in the frozen
//! const order below; a mod pack ADDS a bundle with a namespaced
//! (`mod_id:name`) key, which registers a fresh id in load order.
//!
//! Consumers reference bundles BY KEY, cross-namespace (the same interop rule
//! as effects): a block row's `particle_emitter` may name a bundle instead of
//! carrying an inline row, and mods attach bundles to live mobs through the
//! `MobEmitterSet` HostCall. Body appearance applies to attached entities; a block
//! referencing a bundle just shows its particles.
//!
//! Ids are session-scoped: nothing persists them, and the wire ships the key
//! table at join for remapping (like sounds/effects).
//!
//! An ambient bundle is normally ACTIVATED by a client mod (`ClientAmbientSet`).
//! A biome row may instead list it under `ambient` with a density
//! (`"ambient": {"petramond:butterfly": 0.45}`), which makes the bundle
//! BIOME-DRIVEN: every client derives it wherever the local columns' biomes
//! give it a density, with no mod involved. [`biome_intensity`] is that
//! per-(bundle, biome) table; [`biome_driven`] lists the bundles it covers.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::block::{BlockTag, ParticleEmitter};
use crate::tile::Tile;

/// The biggest cube a row can spawn — the size every visibility cull compares
/// against a pixel, whichever way round the row authored its range.
#[inline]
pub fn particle_size(e: &ParticleEmitter) -> f32 {
    e.size[0].max(e.size[1])
}

/// Engine bundle keys in frozen id order; the completeness oracle
/// `particle_emitters.json` is validated against.
const ENGINE_EMITTER_NAMES: &[&str] = &[
    "petramond:torch_flame",
    "petramond:burn_light",
    "petramond:burn_great",
    "petramond:water_splash",
    "petramond:butterfly",
    "petramond:lava_embers",
];

/// Most particle rows one bundle may declare.
const MAX_BUNDLE_ROWS: usize = 4;

/// One loaded bundle (`defs()[id]`). Exactly one of: LOOPING (`rows`
/// non-empty: shown continuously while attached to a block/mob), a ONE-SHOT
/// `burst` (spawned once per `EmitterBurst` world event, simulated with
/// gravity + collision), or an `ambient` volume (camera-following
/// precipitation, derived statelessly per frame on each client).
pub struct EmitterBundle {
    /// The bundle's session id (its row index).
    pub id: u8,
    /// The registry key (`"petramond:burn_light"`, `"mod_id:sparkle"`).
    pub key: &'static str,
    /// Optional multiply body tint shown while attached to an entity (RGB
    /// `0..=1`). Ignored by block references.
    pub tint: Option<[f32; 3]>,
    /// Authored body dimensions; attached rows scale to their wearer when set.
    pub body_size: Option<[f32; 3]>,
    /// Body light mixed toward full brightness (`0..=1`); attachments compose by max.
    pub body_self_lit: f32,
    /// The looping particle rows, all shown together while the bundle is
    /// active. Empty for a burst or ambient bundle.
    pub rows: &'static [ParticleEmitter],
    /// One-shot burst parameters.
    pub burst: Option<BurstSpec>,
    /// Ambient parameters: a precipitation band, a volume of motes, or a
    /// lattice of fliers (see [`AmbientMotion`]).
    pub ambient: Option<AmbientSpec>,
}

/// A one-shot particle burst: `count_per_intensity × intensity` solid-color
/// flecks (capped) launched upward and outward in a rough circle from the
/// event position, simulated by `entity::ParticleSystem` — real gravity, and
/// (with `die_on_contact`) destroyed the instant they touch a collision box or
/// water. The event's `intensity` is producer-defined; the engine water splash
/// passes the fall distance in blocks.
#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BurstSpec {
    /// Particles per unit of event intensity (result rounded, min 1).
    pub count_per_intensity: f32,
    /// Hard per-burst cap.
    pub max_count: u32,
    /// Min/max upward launch speed, m/s.
    pub up_speed: [f32; 2],
    /// Min/max horizontal launch speed, m/s — each particle picks a random
    /// direction, so the burst spreads in a rough circle.
    pub radial_speed: [f32; 2],
    /// Min/max particle lifetime, seconds.
    pub lifetime: [f32; 2],
    /// Min/max cube edge length, blocks.
    pub size: [f32; 2],
    /// RGB endpoints; each particle draws a mix at spawn.
    pub color: [[f32; 3]; 2],
    /// Skews the color mix: `>1` favors the FIRST endpoint (`mix^bias`), `1`
    /// (default) is uniform.
    #[serde(default = "default_color_bias")]
    pub color_bias: f32,
    /// Destroy the particle the instant it touches a collision box OR water
    /// (default: settle on solids like terrain dust, ignore water).
    #[serde(default)]
    pub die_on_contact: bool,
}

fn default_color_bias() -> f32 {
    1.0
}

/// A world-anchored ambience volume around the local camera, DERIVED
/// statelessly per frame — nothing is simulated, nothing runs on the tick,
/// nothing replicates. Its `motion` decides what a particle IS: a falling
/// drop killed at each column's precipitation ceiling (the topmost
/// movement-blocking or water cell, so nothing falls under a roof), a drifting
/// mote, or a flier orbiting above the ground. Activated per client through the
/// `ClientAmbientSet` host call, or by a biome row's `ambient` density map.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmbientSpec {
    /// Particles at intensity 1.0 (scaled linearly, capped at `max_count`).
    /// Falls and volumes only: a flight's population is its lattice.
    #[serde(default)]
    pub count_per_intensity: f32,
    /// Hard volume cap (falls and volumes only).
    #[serde(default)]
    pub max_count: u32,
    /// Horizontal spawn radius around the camera, blocks.
    pub radius: f32,
    /// Vertical band `[below, above]` the camera the volume covers, blocks.
    pub height: [f32; 2],
    /// Min/max downward fall speed, blocks/s (falls and volumes only).
    #[serde(default)]
    pub fall_speed: [f32; 2],
    /// Multiplier on the activation's wind vector (0 = ignores wind).
    #[serde(default = "default_drift_wind")]
    pub drift_wind: f32,
    /// `[amplitude blocks, hz]` per-particle sinusoidal horizontal wobble
    /// (snowflakes); `[0, 0]` (default) disables (rain).
    #[serde(default)]
    pub flutter: [f32; 2],
    /// Min/max cube edge length, blocks.
    pub size: [f32; 2],
    /// Vertical elongation of the cube (rain streaks); 1 (default) = a cube.
    #[serde(default = "default_stretch")]
    pub stretch: f32,
    /// Min/max particle alpha.
    pub alpha: [f32; 2],
    /// RGB endpoints; each particle draws a mix at birth.
    pub color: [[f32; 3]; 2],
    /// Skews the color mix like a burst's (`>1` favors the first endpoint).
    #[serde(default = "default_color_bias")]
    pub color_bias: f32,
    /// Discrete weighted colours drawn INSTEAD of the `color` mix when
    /// non-empty — for families that must stay distinct (a white, a pink and a
    /// blue butterfly, never a mauve one).
    #[serde(default)]
    pub palette: Vec<PaletteStop>,
    /// What the ceiling hit shows: nothing (`"die"`, default), or a derived
    /// splash from a named BURST bundle's launch/lifetime/color data
    /// (`{"burst": "ns:key"}` — resolved and shape-checked at load).
    #[serde(default)]
    pub hit: AmbientHit,
    /// How a particle MOVES, and therefore what its position is anchored to.
    /// `"precipitation"` (default) falls through the band; `"volume"` is a
    /// drifting body of motes; `{"flight": {...}}` is a lattice of fliers
    /// orbiting above the ground.
    #[serde(default)]
    pub motion: AmbientMotion,
    /// Where a particle STOPS. `"ceiling"` (default) is precipitation: each
    /// column kills at its topmost movement-blocking or water cell, so
    /// nothing falls under a roof. `"interior"` is what a volume that lives
    /// INDOORS (drifting motes, dust, ash) needs instead — no column
    /// ceiling, and a particle is dropped where it would sit inside a wall.
    #[serde(default)]
    pub kill: AmbientKill,
    /// Where a particle's light comes from. `"sky"` (default) is
    /// precipitation: sky-open by construction, so full skylight and the
    /// ordinary sky lanes dim it at night. `"world"` samples the real
    /// skylight + coloured block light at each particle, which is what any
    /// volume that can sit in the dark needs — motes go black in an unlit
    /// pocket and light up next to a lamp.
    #[serde(default)]
    pub light: AmbientLight,
    /// Column-biome filter (at most one of the two, names from the stable
    /// biome vocabulary): particles derive only over columns whose biome is
    /// in `biomes` (or NOT in `exclude_biomes`). How rain and snow draw an
    /// exact side-by-side divide at a biome border — each bundle filters
    /// itself per column; the driving mod runs both.
    #[serde(default)]
    pub biomes: Vec<String>,
    #[serde(default)]
    pub exclude_biomes: Vec<String>,
    /// Resolved at load: 256-bit allow-set over biome ids (`None` = all).
    #[serde(skip)]
    pub biome_allow: Option<[u64; 4]>,
}

/// Whether `biome` passes the resolved allow-set.
#[inline]
pub fn biome_allowed(allow: &Option<[u64; 4]>, biome: u8) -> bool {
    match allow {
        None => true,
        Some(bits) => bits[(biome >> 6) as usize] & (1u64 << (biome & 63)) != 0,
    }
}

fn default_drift_wind() -> f32 {
    1.0
}

/// One entry of an [`AmbientSpec::palette`]: drawn with probability
/// `weight / Σ weights`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteStop {
    pub weight: f32,
    pub color: [f32; 3],
}

/// A lattice of world-anchored FLIERS. Every `spacing`-sized ground cell seeds
/// one candidate (a fraction `occupancy` of them exist at intensity 1); a
/// candidate orbits a closed-form path above the HIGHEST ground its whole
/// orbit covers, so no phase of the flight ever clips a slope. A candidate is
/// refused outright — never lifted — where any column under its orbit is
/// unloaded, roofed by a non-ground block (a canopy, a slab, a pane of glass),
/// excluded by the bundle's biome filter, or rolls above the biome's density.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlightSpec {
    /// Horizontal lattice pitch, blocks — one candidate per cell.
    pub spacing: f32,
    /// Fraction of lattice cells occupied at intensity 1.
    pub occupancy: f32,
    /// Orbit half-extents `[x, z]` around the cell's anchor, blocks.
    pub orbit: [f32; 2],
    /// Cruise height above the anchor ground `[base, bob amplitude]`, blocks.
    pub hover: [f32; 2],
    /// Min/max orbit rate, radians/s.
    pub speed: [f32; 2],
    /// Min/max wingbeat rate, Hz (used with `sprite`).
    #[serde(default)]
    pub flap_hz: [f32; 2],
    /// Atlas tile whose left and right halves are the two wings, hinged at the
    /// body and flapping. Absent: the ordinary solid particle cube.
    #[serde(default)]
    pub sprite: Option<String>,
    /// Block tags, ANY of which the ground under the orbit must carry (empty:
    /// any ground). Butterflies list `soil` so a canopy is an obstruction, not
    /// a floor.
    #[serde(default)]
    pub ground_tags: Vec<String>,
    /// `sprite` resolved at load.
    #[serde(skip)]
    pub sprite_tile: Option<Tile>,
    /// `ground_tags` resolved at load.
    #[serde(skip)]
    pub ground: Vec<BlockTag>,
}

fn default_stretch() -> f32 {
    1.0
}

/// An ambient particle's ceiling-hit behavior.
#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientHit {
    /// Disappear silently.
    #[default]
    Die,
    /// Show a stateless splash derived from this BURST bundle's data.
    Burst(String),
}

/// How an ambient volume's particles are anchored and move.
///
/// Every kind is world-anchored: the body follows the player without dragging
/// its contents along (falls and volumes wrap positions into the camera's box;
/// fliers live on a fixed ground lattice).
#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientMotion {
    /// Falling particles reseed their column when recycling across the
    /// vertical band and can derive splashes at their world-space hit time.
    #[default]
    Precipitation,
    /// A BODY of drifting motes: all three axes are world-anchored, and Y
    /// wraps into the `height` band exactly as X/Z wrap into `radius`. Without
    /// this the band re-centres on the camera every frame and the whole field
    /// rides the player's jump.
    Volume,
    /// A lattice of fliers orbiting above the ground — see [`FlightSpec`].
    Flight(FlightSpec),
}

impl AmbientMotion {
    /// The flight parameters, for the kind that has them.
    pub fn flight(&self) -> Option<&FlightSpec> {
        match self {
            AmbientMotion::Flight(f) => Some(f),
            _ => None,
        }
    }
}

/// Where an ambient volume's particles stop.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientKill {
    /// At the column's precipitation ceiling (topmost movement-blocking or
    /// water cell) — falls on roofs and lakes, never under them.
    #[default]
    Ceiling,
    /// No column ceiling at all; a particle is dropped only where the cell it
    /// occupies blocks movement. The interior twin of the ceiling rule: every
    /// enclosed space in the world sits UNDER some column's ceiling, so a
    /// precipitation volume can never show indoors however it is tuned, and
    /// what an indoor volume actually needs is "not inside the wall".
    Interior,
}

/// Where an ambient volume's particles take their light.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientLight {
    /// Full skylight — correct for anything that only exists under open sky.
    #[default]
    Sky,
    /// The world's own sampled skylight + coloured block light.
    World,
}

/// One bundle row as written in `particle_emitters.json`: exactly one of
/// `particles` (looping), `burst` (one-shot), or `ambient` (camera volume).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundle {
    emitter: String,
    #[serde(default)]
    tint: Option<[f32; 3]>,
    #[serde(default)]
    body_size: Option<[f32; 3]>,
    #[serde(default)]
    body_self_lit: f32,
    #[serde(default)]
    particles: Vec<ParticleEmitter>,
    #[serde(default)]
    burst: Option<BurstSpec>,
    #[serde(default)]
    ambient: Option<AmbientSpec>,
}

#[derive(Deserialize)]
struct RawFile {
    emitters: Vec<RawBundle>,
}

/// The bundle registered under `key`, or `None` when no such row is loaded.
pub fn by_key(key: &str) -> Option<&'static EmitterBundle> {
    catalog().id(key).map(|id| &catalog().rows()[id as usize])
}

/// The bundle with session id `id`, or `None` for an unregistered id.
pub fn def(id: u8) -> Option<&'static EmitterBundle> {
    defs().get(id as usize)
}

/// The loaded bundle table, id-ordered. Loads exactly once; a missing or
/// inconsistent `particle_emitters.json` fails loudly at startup.
pub fn defs() -> &'static [EmitterBundle] {
    catalog().rows()
}

fn catalog() -> &'static crate::registry::Catalog<EmitterBundle> {
    &tables().0
}

/// The bundle catalog plus the biome-density table over it. Both fail loudly
/// at first use, like every catalog: a biome row naming a bundle that does not
/// exist (or is not ambient) is a content error, not a silent no-show.
fn tables() -> &'static (crate::registry::Catalog<EmitterBundle>, BiomeTable) {
    static TABLES: LazyLock<(crate::registry::Catalog<EmitterBundle>, BiomeTable)> =
        LazyLock::new(|| {
            let catalog =
                crate::registry::read_catalog("particle_emitters.json", "emitter", parse_layers);
            let rows = (1..=crate::biome::BIOME_COUNT as u8)
                .map(|id| (id, crate::biome::Biome::from_id(id).ambient()));
            let table = BiomeTable::build(&catalog, rows)
                .unwrap_or_else(|e| panic!("biomes.json ambient densities: {e}"));
            (catalog, table)
        });
    &TABLES
}

/// The density at which `biome` drives `bundle` — the biome row's `ambient`
/// entry, `0` for a biome that omits a biome-driven bundle, and `1` for a
/// bundle no biome row names at all (a mod-driven bundle is not thinned by
/// biome unless it declares its own filter).
#[inline]
pub fn biome_intensity(bundle: u8, biome: u8) -> f32 {
    tables().1.intensity(bundle, biome)
}

/// The bundles some biome row drives, id-ordered. Every client derives these
/// every frame; no mod activation is involved.
pub fn biome_driven() -> &'static [u8] {
    &tables().1.driven
}

/// Per-(bundle, biome) density, dense over the byte id spaces.
struct BiomeTable {
    /// `cells[bundle * 256 + biome]`; rows of bundles no biome names are all 1.
    cells: Box<[f32]>,
    driven: Box<[u8]>,
}

impl BiomeTable {
    fn build<'a>(
        catalog: &crate::registry::Catalog<EmitterBundle>,
        biome_rows: impl Iterator<Item = (u8, &'a [(&'a str, f32)])>,
    ) -> Result<Self, String> {
        let bundles = catalog.rows().len();
        let mut cells = vec![1.0f32; bundles * 256].into_boxed_slice();
        let mut driven = Vec::new();
        for (biome, entries) in biome_rows {
            for &(key, density) in entries {
                let Some(id) = catalog.id(key) else {
                    return Err(format!(
                        "biome {biome} names unknown emitter bundle '{key}'"
                    ));
                };
                let id = id as usize;
                if catalog.rows()[id].ambient.is_none() {
                    return Err(format!(
                        "biome {biome} names '{key}', which is not an ambient bundle"
                    ));
                }
                if !driven.contains(&(id as u8)) {
                    driven.push(id as u8);
                    cells[id * 256..(id + 1) * 256].fill(0.0);
                }
                cells[id * 256 + biome as usize] = density;
            }
        }
        driven.sort_unstable();
        Ok(Self {
            cells,
            driven: driven.into_boxed_slice(),
        })
    }

    #[inline]
    fn intensity(&self, bundle: u8, biome: u8) -> f32 {
        self.cells
            .get(bundle as usize * 256 + biome as usize)
            .copied()
            .unwrap_or(1.0)
    }
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<EmitterBundle>, String> {
    let catalog = crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.emitters),
        |r| &r.emitter,
        ENGINE_EMITTER_NAMES,
        "emitter",
        |mut r, id, names| {
            let kinds = usize::from(!r.particles.is_empty())
                + usize::from(r.burst.is_some())
                + usize::from(r.ambient.is_some());
            if kinds != 1 {
                return Err(format!(
                    "emitter '{}': declare exactly one of particles (looping), burst (one-shot), or ambient (camera volume)",
                    r.emitter
                ));
            }
            if r.particles.len() > MAX_BUNDLE_ROWS {
                return Err(format!(
                    "emitter '{}': 1..={MAX_BUNDLE_ROWS} particle rows per bundle, got {}",
                    r.emitter,
                    r.particles.len()
                ));
            }
            if let Some(burst) = &r.burst {
                validate_burst(&r.emitter, burst)?;
            }
            if let Some(ambient) = &mut r.ambient {
                validate_ambient(&r.emitter, ambient)?;
                ambient.biome_allow = resolve_biome_filter(&r.emitter, ambient)?;
                if let AmbientMotion::Flight(flight) = &mut ambient.motion {
                    resolve_flight(&r.emitter, flight)?;
                }
            }
            if let Some(size) = r.body_size {
                if r.particles.is_empty() || size.iter().any(|v| !v.is_finite() || *v <= 0.0) {
                    return Err(format!(
                        "emitter '{}': body_size requires looping rows and positive dimensions",
                        r.emitter
                    ));
                }
            }
            if let Some(tint) = r.tint {
                for channel in tint {
                    if !channel.is_finite() || !(0.0..=1.0).contains(&channel) {
                        return Err(format!(
                            "emitter '{}': tint channels must be in 0..=1",
                            r.emitter
                        ));
                    }
                }
            }
            if !r.body_self_lit.is_finite() || !(0.0..=1.0).contains(&r.body_self_lit) {
                return Err(format!(
                    "emitter '{}': body_self_lit must be in 0..=1",
                    r.emitter
                ));
            }
            for particle in &r.particles {
                crate::block::validate_particle_emitter(particle)
                    .map_err(|e| format!("emitter '{}': {e}", r.emitter))?;
            }
            Ok(EmitterBundle {
                id: id as u8,
                key: names.name(id).expect("id resolved from this table"),
                tint: r.tint,
                body_size: r.body_size,
                body_self_lit: r.body_self_lit,
                rows: Box::leak(r.particles.into_boxed_slice()),
                burst: r.burst,
                ambient: r.ambient,
            })
        },
    )?;
    // Cross-bundle references resolve against the FINISHED table (an ambient
    // may name a burst declared by any pack, in any load order).
    for row in catalog.rows() {
        if let Some(AmbientSpec {
            hit: AmbientHit::Burst(key),
            ..
        }) = &row.ambient
        {
            match catalog.id(key).map(|id| &catalog.rows()[id as usize]) {
                Some(target) if target.burst.is_some() => {}
                Some(_) => {
                    return Err(format!(
                        "emitter '{}': ambient hit '{key}' is not a burst bundle",
                        row.key
                    ))
                }
                None => {
                    return Err(format!(
                        "emitter '{}': ambient hit names unknown bundle '{key}'",
                        row.key
                    ))
                }
            }
        }
    }
    Ok(catalog)
}

/// Resolve the row's biome filter to an id bitset against the stable
/// vocabulary. Unknown names and declaring BOTH list kinds are load errors.
fn resolve_biome_filter(key: &str, a: &AmbientSpec) -> Result<Option<[u64; 4]>, String> {
    if a.biomes.is_empty() && a.exclude_biomes.is_empty() {
        return Ok(None);
    }
    if !a.biomes.is_empty() && !a.exclude_biomes.is_empty() {
        return Err(format!(
            "emitter '{key}' ambient: declare biomes OR exclude_biomes, not both"
        ));
    }
    let exclude = !a.exclude_biomes.is_empty();
    let names = if exclude {
        &a.exclude_biomes
    } else {
        &a.biomes
    };
    let mut bits = if exclude { [u64::MAX; 4] } else { [0u64; 4] };
    for name in names {
        let Some(id) = mod_api::biome::by_name(name) else {
            return Err(format!("emitter '{key}' ambient: unknown biome '{name}'"));
        };
        let (word, bit) = ((id >> 6) as usize, 1u64 << (id & 63));
        if exclude {
            bits[word] &= !bit;
        } else {
            bits[word] |= bit;
        }
    }
    Ok(Some(bits))
}

fn validate_ambient(key: &str, a: &AmbientSpec) -> Result<(), String> {
    let err = |what: &str| Err(format!("emitter '{key}' ambient: {what}"));
    match &a.motion {
        AmbientMotion::Flight(flight) => {
            if a.count_per_intensity != 0.0 || a.max_count != 0 || a.fall_speed != [0.0, 0.0] {
                return err(
                    "a flight's population is spacing × occupancy and it never falls: \
                     omit count_per_intensity, max_count and fall_speed",
                );
            }
            if a.flutter != [0.0, 0.0] || a.stretch != 1.0 || a.hit != AmbientHit::Die {
                return err("flutter, stretch and hit do not apply to a flight");
            }
            validate_flight(key, flight)?;
        }
        AmbientMotion::Precipitation | AmbientMotion::Volume => {
            if !a.count_per_intensity.is_finite() || a.count_per_intensity <= 0.0 {
                return err("count_per_intensity must be positive and finite");
            }
            if !(1..=4096).contains(&a.max_count) {
                return err("max_count must be in 1..=4096");
            }
            if !a.fall_speed[0].is_finite()
                || !a.fall_speed[1].is_finite()
                || a.fall_speed[0] < 0.1
                || a.fall_speed[0] > a.fall_speed[1]
            {
                return err("fall_speed must be a finite ordered range (min 0.1)");
            }
        }
    }
    if !a.radius.is_finite() || !(4.0..=48.0).contains(&a.radius) {
        return err("radius must be in 4..=48");
    }
    for half in a.height {
        if !half.is_finite() || !(0.0..=64.0).contains(&half) {
            return err("height band values must be in 0..=64");
        }
    }
    if a.height[0] + a.height[1] <= 0.0 {
        return err("the height band must have positive extent");
    }
    for (label, range, min) in [("size", a.size, f32::EPSILON), ("alpha", a.alpha, 0.0)] {
        if !range[0].is_finite() || !range[1].is_finite() || range[0] < min || range[0] > range[1] {
            return Err(format!(
                "emitter '{key}' ambient: {label} must be a finite ordered range (min {min})"
            ));
        }
    }
    if a.alpha[1] > 1.0 || a.size[1] > 1.0 {
        return err("alpha and size must stay at or below 1");
    }
    if !a.drift_wind.is_finite() || !(0.0..=4.0).contains(&a.drift_wind) {
        return err("drift_wind must be in 0..=4");
    }
    if !a.flutter[0].is_finite()
        || !a.flutter[1].is_finite()
        || !(0.0..=4.0).contains(&a.flutter[0])
        || !(0.0..=8.0).contains(&a.flutter[1])
    {
        return err("flutter must be [amplitude 0..=4, hz 0..=8]");
    }
    if !a.stretch.is_finite() || !(1.0..=16.0).contains(&a.stretch) {
        return err("stretch must be in 1..=16");
    }
    for stop in a.color {
        for channel in stop {
            if !channel.is_finite() || !(0.0..=1.0).contains(&channel) {
                return err("color channels must be in 0..=1");
            }
        }
    }
    if !a.color_bias.is_finite() || !(0.25..=8.0).contains(&a.color_bias) {
        return err("color_bias must be in 0.25..=8");
    }
    for stop in &a.palette {
        if !stop.weight.is_finite() || stop.weight <= 0.0 {
            return err("palette weights must be positive and finite");
        }
        for channel in stop.color {
            if !channel.is_finite() || !(0.0..=1.0).contains(&channel) {
                return err("palette color channels must be in 0..=1");
            }
        }
    }
    // A splash is derived AT the kill height; an interior volume has no
    // impact plane to derive it at, so the pair is a load error rather than a
    // silently ignored field.
    if a.kill == AmbientKill::Interior && a.hit != AmbientHit::Die {
        return err("kill 'interior' has no impact point, so hit must be 'die'");
    }
    // A splash is solved from the fall's cycle — when the drop crossed the
    // column's kill height. A world-anchored volume has no fall to solve, so
    // the pair is a load error rather than a silently wrong crown.
    if a.motion == AmbientMotion::Volume && a.hit != AmbientHit::Die {
        return err("motion 'volume' does not fall onto anything, so hit must be 'die'");
    }
    Ok(())
}

fn validate_flight(key: &str, f: &FlightSpec) -> Result<(), String> {
    let err = |what: &str| Err(format!("emitter '{key}' flight: {what}"));
    if !f.spacing.is_finite() || !(1.0..=32.0).contains(&f.spacing) {
        return err("spacing must be in 1..=32");
    }
    if !f.occupancy.is_finite() || !(0.0..=1.0).contains(&f.occupancy) || f.occupancy == 0.0 {
        return err("occupancy must be in (0, 1]");
    }
    for (label, pair, max) in [
        ("orbit", f.orbit, 16.0),
        ("hover", f.hover, 16.0),
        ("flap_hz", f.flap_hz, 32.0),
    ] {
        if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] < 0.0 || pair[1] > max {
            return Err(format!(
                "emitter '{key}' flight: {label} values must be finite and in 0..={max}"
            ));
        }
    }
    if f.flap_hz[0] > f.flap_hz[1] {
        return err("flap_hz must be an ordered range");
    }
    if !f.speed[0].is_finite()
        || !f.speed[1].is_finite()
        || f.speed[0] <= 0.0
        || f.speed[0] > f.speed[1]
        || f.speed[1] > 16.0
    {
        return err("speed must be a positive ordered range at or below 16 rad/s");
    }
    if f.orbit[0].max(f.orbit[1]) >= f.spacing * 0.5 {
        return err("orbit half-extents must stay inside half the spacing");
    }
    Ok(())
}

/// Resolve a flight's names against the tile manifest and the block-tag
/// vocabulary — both load-time facts, so a typo is a load error.
fn resolve_flight(key: &str, f: &mut FlightSpec) -> Result<(), String> {
    if let Some(name) = &f.sprite {
        let Some(tile) = Tile::from_name(name) else {
            return Err(format!(
                "emitter '{key}' flight: sprite names no tile '{name}'"
            ));
        };
        f.sprite_tile = Some(tile);
    }
    f.ground = f
        .ground_tags
        .iter()
        .map(|t| BlockTag::resolve(t).map_err(|e| format!("emitter '{key}' flight: {e}")))
        .collect::<Result<_, _>>()?;
    Ok(())
}

fn validate_burst(key: &str, b: &BurstSpec) -> Result<(), String> {
    let err = |what: &str| Err(format!("emitter '{key}' burst: {what}"));
    if !b.count_per_intensity.is_finite() || b.count_per_intensity <= 0.0 {
        return err("count_per_intensity must be positive and finite");
    }
    if !(1..=256).contains(&b.max_count) {
        return err("max_count must be in 1..=256");
    }
    for (label, range, min) in [
        ("up_speed", b.up_speed, 0.0),
        ("radial_speed", b.radial_speed, 0.0),
        ("lifetime", b.lifetime, f32::EPSILON),
        ("size", b.size, f32::EPSILON),
    ] {
        if !range[0].is_finite() || !range[1].is_finite() || range[0] < min || range[0] > range[1] {
            return Err(format!(
                "emitter '{key}' burst: {label} must be a finite ordered non-negative range"
            ));
        }
    }
    for stop in b.color {
        for channel in stop {
            if !channel.is_finite() || !(0.0..=1.0).contains(&channel) {
                return err("color channels must be in 0..=1");
            }
        }
    }
    if !b.color_bias.is_finite() || !(0.25..=8.0).contains(&b.color_bias) {
        return err("color_bias must be in 0.25..=8");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
