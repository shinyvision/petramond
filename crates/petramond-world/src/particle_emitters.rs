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

use serde::Deserialize;

use crate::block::{BlockTag, ParticleEmitter};
use crate::tile::Tile;

#[inline]
pub fn particle_size(e: &ParticleEmitter) -> f32 {
    e.size[0].max(e.size[1])
}

const ENGINE_EMITTER_NAMES: &[&str] = &[
    "petramond:torch_flame",
    "petramond:burn_light",
    "petramond:burn_great",
    "petramond:water_splash",
    "petramond:butterfly",
    "petramond:lava_embers",
    "petramond:block_dust",
    "petramond:block_break",
];

const MAX_BUNDLE_ROWS: usize = 4;

pub struct EmitterBundle {
    pub id: u8,
    pub key: &'static str,
    pub tint: Option<[f32; 3]>,
    pub body_size: Option<[f32; 3]>,
    pub body_self_lit: f32,
    pub rows: &'static [ParticleEmitter],
    pub burst: Option<BurstSpec>,
    pub ambient: Option<AmbientSpec>,
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BurstSpec {
    pub count_per_intensity: f32,
    pub up_speed: [f32; 2],
    pub radial_speed: [f32; 2],
    pub lifetime: [f32; 2],
    pub size: [f32; 2],
    #[serde(default)]
    pub count_spread: f32,
    #[serde(default = "default_spawn")]
    pub spawn: [f32; 3],
    #[serde(default)]
    pub outward_speed: [f32; 2],
    #[serde(default)]
    pub along_speed: [f32; 2],
    #[serde(default = "default_color")]
    pub color: [[f32; 3]; 2],
    #[serde(default)]
    pub texture: Option<TextureSlice>,
    #[serde(default = "default_patch")]
    pub patch: f32,
    #[serde(default = "default_color_bias")]
    pub color_bias: f32,
    #[serde(default)]
    pub die_on_contact: bool,
}

fn default_color_bias() -> f32 {
    1.0
}

fn default_spawn() -> [f32; 3] {
    [0.15, 0.05, 0.15]
}

fn default_color() -> [[f32; 3]; 2] {
    [[1.0; 3]; 2]
}

fn default_patch() -> f32 {
    0.25
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "RawTextureSlice")]
pub struct TextureSlice {
    pub tile: crate::tile::Tile,
    pub slice: [f32; 4],
}

impl TextureSlice {
    pub const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    pub fn named(tile: &str, slice: [f32; 4]) -> Option<Self> {
        let [u0, v0, u1, v1] = slice;
        let inside = slice
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v));
        (inside && u0 < u1 && v0 < v1).then_some(Self {
            tile: crate::tile::Tile::from_name(tile)?,
            slice,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTextureSlice {
    tile: String,
    #[serde(default = "whole_slice")]
    slice: [f32; 4],
}

fn whole_slice() -> [f32; 4] {
    TextureSlice::WHOLE
}

impl TryFrom<RawTextureSlice> for TextureSlice {
    type Error = String;

    fn try_from(raw: RawTextureSlice) -> Result<Self, String> {
        Self::named(&raw.tile, raw.slice)
            .ok_or_else(|| format!("texture '{}': unknown tile or a slice outside it", raw.tile))
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmbientSpec {
    #[serde(default)]
    pub count_per_intensity: f32,
    #[serde(default)]
    pub max_count: u32,
    pub radius: f32,
    pub height: [f32; 2],
    #[serde(default)]
    pub fall_speed: [f32; 2],
    #[serde(default = "default_drift_wind")]
    pub drift_wind: f32,
    #[serde(default)]
    pub flutter: [f32; 2],
    pub size: [f32; 2],
    #[serde(default = "default_stretch")]
    pub stretch: f32,
    pub alpha: [f32; 2],
    pub color: [[f32; 3]; 2],
    #[serde(default = "default_color_bias")]
    pub color_bias: f32,
    #[serde(default)]
    pub palette: Vec<PaletteStop>,
    #[serde(default)]
    pub hit: AmbientHit,
    #[serde(default)]
    pub motion: AmbientMotion,
    #[serde(default)]
    pub kill: AmbientKill,
    #[serde(default)]
    pub light: AmbientLight,
    #[serde(default)]
    pub biomes: Vec<String>,
    #[serde(default)]
    pub exclude_biomes: Vec<String>,
    #[serde(skip)]
    pub biome_allow: Option<[u64; 4]>,
}

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

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteStop {
    pub weight: f32,
    pub color: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlightSpec {
    pub spacing: f32,
    pub occupancy: f32,
    pub orbit: [f32; 2],
    pub hover: [f32; 2],
    pub speed: [f32; 2],
    #[serde(default)]
    pub flap_hz: [f32; 2],
    #[serde(default)]
    pub sprite: Option<String>,
    #[serde(default)]
    pub ground_tags: Vec<String>,
    #[serde(skip)]
    pub sprite_tile: Option<Tile>,
    #[serde(skip)]
    pub ground: Vec<BlockTag>,
}

fn default_stretch() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientHit {
    #[default]
    Die,
    Burst(String),
}

#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientMotion {
    #[default]
    Precipitation,
    Volume,
    Flight(FlightSpec),
}

impl AmbientMotion {
    pub fn flight(&self) -> Option<&FlightSpec> {
        match self {
            AmbientMotion::Flight(f) => Some(f),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientKill {
    #[default]
    Ceiling,
    Interior,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbientLight {
    #[default]
    Sky,
    World,
}

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

pub fn by_key(key: &str) -> Option<&'static EmitterBundle> {
    catalog().id(key).map(|id| &catalog().rows()[id as usize])
}

pub fn def(id: u8) -> Option<&'static EmitterBundle> {
    defs().get(id as usize)
}

pub fn defs() -> &'static [EmitterBundle] {
    catalog().rows()
}

fn catalog() -> &'static crate::registry::Catalog<EmitterBundle> {
    &tables().0
}

pub(crate) static TABLES: crate::content::Slot<(
    crate::registry::Catalog<EmitterBundle>,
    BiomeTable,
)> = crate::content::Slot::new(
    crate::content::stage::PARTICLE_EMITTERS,
    &[
        crate::content::stage::TILES,
        crate::content::stage::SOUNDS,
        crate::content::stage::BIOMES,
    ],
    load,
);

fn load(
    reg: &crate::content::ContentRegistry,
) -> Result<(crate::registry::Catalog<EmitterBundle>, BiomeTable), String> {
    let catalog = crate::registry::read_catalog(
        reg.packs(),
        "particle_emitters.json",
        "emitter",
        parse_layers,
    )?;
    let rows = crate::biome::Biome::all().map(|biome| (biome.id(), biome.ambient()));
    let table = BiomeTable::build(&catalog, rows)
        .map_err(|e| format!("biomes.json ambient densities: {e}"))?;
    Ok((catalog, table))
}

fn tables() -> &'static (crate::registry::Catalog<EmitterBundle>, BiomeTable) {
    TABLES.current()
}

#[inline]
pub fn biome_intensity(bundle: u8, biome: u8) -> f32 {
    tables().1.intensity(bundle, biome)
}

pub fn biome_driven() -> &'static [u8] {
    &tables().1.driven
}

pub(crate) struct BiomeTable {
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
    if a.kill == AmbientKill::Interior && a.hit != AmbientHit::Die {
        return err("kill 'interior' has no impact point, so hit must be 'die'");
    }
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
    for (label, range, min) in [
        ("up_speed", b.up_speed, 0.0),
        ("radial_speed", b.radial_speed, 0.0),
        ("outward_speed", b.outward_speed, 0.0),
        ("along_speed", b.along_speed, 0.0),
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
    if !b.count_spread.is_finite() || !(0.0..=4.0).contains(&b.count_spread) {
        return err("count_spread must be in 0..=4");
    }
    if b.spawn
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=2.0).contains(v))
    {
        return err("spawn half extents must be in 0..=2");
    }
    if !b.patch.is_finite() || !(0.01..=1.0).contains(&b.patch) {
        return err("patch must be in 0.01..=1");
    }
    if !b.color_bias.is_finite() || !(0.25..=8.0).contains(&b.color_bias) {
        return err("color_bias must be in 0.25..=8");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
