//! Keyed particle-emitter bundles: the layered `particle_emitters.json`
//! catalog (a catalog like `effects.json` — see [`crate::effect`] for the
//! pattern).
//!
//! A BUNDLE is one named visual effect: one or more particle rows (the shared
//! [`ParticleEmitter`] schema blocks use) plus an
//! optional multiply body tint. Engine bundles own the low ids in the frozen
//! const order below; a mod pack ADDS a bundle with a namespaced
//! (`mod_id:name`) key, which registers a fresh id in load order.
//!
//! Consumers reference bundles BY KEY, cross-namespace (the same interop rule
//! as effects): a block row's `particle_emitter` may name a bundle instead of
//! carrying an inline row, and mods attach bundles to live mobs through the
//! `MobEmitterSet` HostCall. Tint applies to mob bodies only; a block
//! referencing a tinted bundle just shows its particles.
//!
//! Ids are session-scoped: nothing persists them, and the wire ships the key
//! table at join for remapping (like sounds/effects).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::block::ParticleEmitter;

/// Engine bundle keys in frozen id order; the completeness oracle
/// `particle_emitters.json` is validated against.
const ENGINE_EMITTER_NAMES: &[&str] = &[
    "petramond:torch_flame",
    "petramond:burn_light",
    "petramond:burn_great",
    "petramond:water_splash",
];

/// The engine water-splash burst bundle, emitted by core physics when a player
/// or mob FALLS into water (see `ServerGame::push_water_splash`).
pub const WATER_SPLASH_KEY: &str = "petramond:water_splash";

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
    /// Optional multiply body tint shown while attached to a mob (RGB
    /// `0..=1`). Ignored by block references.
    pub tint: Option<[f32; 3]>,
    /// The looping particle rows, all shown together while the bundle is
    /// active. Empty for a burst or ambient bundle.
    pub rows: &'static [ParticleEmitter],
    /// One-shot burst parameters.
    pub burst: Option<BurstSpec>,
    /// Camera-volume parameters.
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

/// A camera-following ambience/precipitation volume: up to
/// `count_per_intensity × intensity` cubes (capped) DERIVED statelessly per
/// frame around the local camera — falling at `fall_speed`, advected by the
/// activation's wind, fluttering if asked — and killed at each column's
/// precipitation ceiling (the topmost movement-blocking or water cell), so
/// nothing falls under a roof and hits land ON the roof. Activated per client
/// through the `ClientAmbientSet` host call; never simulated, never on the
/// tick, never replicated.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmbientSpec {
    /// Particles at intensity 1.0 (scaled linearly, capped at `max_count`).
    pub count_per_intensity: f32,
    /// Hard volume cap.
    pub max_count: u32,
    /// Horizontal spawn radius around the camera, blocks.
    pub radius: f32,
    /// Vertical band `[below, above]` the camera the volume covers, blocks.
    pub height: [f32; 2],
    /// Min/max downward fall speed, blocks/s.
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
    /// What the ceiling hit shows: nothing (`"die"`, default), or a derived
    /// splash from a named BURST bundle's launch/lifetime/color data
    /// (`{"burst": "ns:key"}` — resolved and shape-checked at load).
    #[serde(default)]
    pub hit: AmbientHit,
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

/// One bundle row as written in `particle_emitters.json`: exactly one of
/// `particles` (looping), `burst` (one-shot), or `ambient` (camera volume).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundle {
    emitter: String,
    #[serde(default)]
    tint: Option<[f32; 3]>,
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
    static TABLE: LazyLock<crate::registry::Catalog<EmitterBundle>> = LazyLock::new(|| {
        crate::registry::read_catalog("particle_emitters.json", "emitter", parse_layers)
    });
    &TABLE
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
            for particle in &r.particles {
                crate::block::validate_particle_emitter(particle)
                    .map_err(|e| format!("emitter '{}': {e}", r.emitter))?;
            }
            Ok(EmitterBundle {
                id,
                key: names.name(id).expect("id resolved from this table"),
                tint: r.tint,
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
    if !a.count_per_intensity.is_finite() || a.count_per_intensity <= 0.0 {
        return err("count_per_intensity must be positive and finite");
    }
    if !(1..=4096).contains(&a.max_count) {
        return err("max_count must be in 1..=4096");
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
    for (label, range, min) in [
        ("fall_speed", a.fall_speed, 0.1),
        ("size", a.size, f32::EPSILON),
        ("alpha", a.alpha, 0.0),
    ] {
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
mod tests {
    use super::*;

    fn base() -> String {
        let (text, _) = crate::assets::read_base_text("particle_emitters.json")
            .expect("assets/particle_emitters.json must ship");
        text
    }

    /// The shipped catalog must load fully — the startup gate as a test.
    #[test]
    fn shipped_particle_emitters_json_loads_fully() {
        let defs = parse_layers(&[&base()])
            .unwrap_or_else(|e| panic!("shipped catalog: {e}"))
            .rows();
        assert_eq!(defs.len(), ENGINE_EMITTER_NAMES.len());
        for (i, d) in defs.iter().enumerate() {
            assert_eq!(d.id, i as u8);
            assert_eq!(d.key, ENGINE_EMITTER_NAMES[i]);
            let kinds = usize::from(!d.rows.is_empty())
                + usize::from(d.burst.is_some())
                + usize::from(d.ambient.is_some());
            assert_eq!(kinds, 1, "every bundle is exactly one kind");
        }
        assert!(
            by_key(WATER_SPLASH_KEY).unwrap().burst.is_some(),
            "the water splash ships as a burst"
        );
    }

    #[test]
    fn burst_bundles_validate() {
        // (count, max, up_speed, bias) — the fields the bad cases vary.
        let splash = |count: &str, max: &str, up: &str, bias: &str| {
            format!(
                r#"{{"emitters": [{{"emitter": "mymod:pop", "burst": {{
                    "count_per_intensity": {count}, "max_count": {max},
                    "up_speed": {up}, "radial_speed": [0.5, 1.5],
                    "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                    "color": [[0.1, 0.1, 0.5], [0.4, 0.8, 1.0]],
                    "color_bias": {bias}, "die_on_contact": true }} }}]}}"#
            )
        };
        let ok = splash("3.0", "24", "[1.0, 2.0]", "2.0");
        let defs = parse_layers(&[&base(), ok.as_str()])
            .expect("burst bundle loads")
            .rows();
        let d = defs.last().unwrap();
        assert!(d.burst.is_some() && d.rows.is_empty());

        for (bad, why) in [
            (
                splash("0.0", "24", "[1.0, 2.0]", "2.0"),
                "zero count scaling",
            ),
            (splash("3.0", "0", "[1.0, 2.0]", "2.0"), "zero cap"),
            (splash("3.0", "24", "[2.0, 1.0]", "2.0"), "reversed range"),
            (
                splash("3.0", "24", "[1.0, 2.0]", "100.0"),
                "out-of-range bias",
            ),
            (
                r#"{"emitters": [{"emitter": "mymod:pop", "burst": {
                    "count_per_intensity": 3.0, "max_count": 24,
                    "up_speed": [1.0, 2.0], "radial_speed": [0.5, 1.5],
                    "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                    "color": [[0.1, 0.1, 0.5], [0.4, 0.8, 1.0]] },
                    "particles": [{"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                        "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}] }]}"#
                    .to_owned(),
                "both burst and particles",
            ),
        ] {
            assert!(
                parse_layers(&[&base(), bad.as_str()]).is_err(),
                "{why} must fail the load"
            );
        }
    }

    #[test]
    fn pack_bundles_register_after_engine_rows_and_validate() {
        let glow = r#"{"emitter": "mymod:glow", "tint": [1.0, 0.9, 0.6], "particles": [
            {"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
             "color": [[0.9, 0.9, 0.2], [1.0, 1.0, 0.6]], "alpha": [0.5, 0.8]}]}"#;
        let pack = format!(r#"{{"emitters": [{glow}]}}"#);
        let defs = parse_layers(&[&base(), pack.as_str()])
            .expect("pack bundle loads")
            .rows();
        let d = defs.last().unwrap();
        assert_eq!(d.key, "mymod:glow");
        assert_eq!(d.tint, Some([1.0, 0.9, 0.6]));
        assert_eq!(d.rows.len(), 1);

        for (bad, why) in [
            (
                r#"{"emitter": "mymod:glow", "particles": []}"#.to_owned(),
                "no particle rows",
            ),
            (
                r#"{"emitter": "mymod:glow", "tint": [2.0, 0.0, 0.0], "particles": [
                    {"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                     "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}]}"#
                    .to_owned(),
                "out-of-range tint",
            ),
            (
                r#"{"emitter": "mymod:glow", "particles": [
                    {"rate": 2.0, "lifetime": [0.8, 0.4], "size": [0.05, 0.1],
                     "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}]}"#
                    .to_owned(),
                "a row failing shared emitter validation",
            ),
            (
                r#"{"emitter": "bareglow", "particles": [
                    {"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                     "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}]}"#
                    .to_owned(),
                "a bare (un-namespaced) new key",
            ),
        ] {
            let pack = format!(r#"{{"emitters": [{bad}]}}"#);
            assert!(
                parse_layers(&[&base(), pack.as_str()]).is_err(),
                "{why} must fail the load"
            );
        }
    }

    #[test]
    fn ambient_biome_filters_resolve_and_validate() {
        let with_filter = |extra: &str| {
            format!(
                r#"{{"emitters": [{{"emitter": "mymod:ashfall", "ambient": {{
                    "count_per_intensity": 100, "max_count": 200, "radius": 16,
                    "height": [4, 16], "fall_speed": [2, 4],
                    "size": [0.05, 0.08], "alpha": [0.5, 0.8],
                    "color": [[0.3, 0.3, 0.3], [0.5, 0.5, 0.5]]{extra} }} }}]}}"#
            )
        };
        // Allow-list resolves to a set admitting exactly its members.
        let ok = with_filter(r#", "biomes": ["snowy_plains", "snowy_taiga"]"#);
        let defs = parse_layers(&[&base(), ok.as_str()]).expect("filter loads");
        let allow = &defs
            .rows()
            .last()
            .unwrap()
            .ambient
            .as_ref()
            .unwrap()
            .biome_allow;
        assert!(allow.is_some());
        assert!(biome_allowed(allow, mod_api::biome::SNOWY_PLAINS));
        assert!(!biome_allowed(allow, mod_api::biome::PLAINS));
        // Exclusion admits the complement.
        let ok = with_filter(r#", "exclude_biomes": ["desert"]"#);
        let defs = parse_layers(&[&base(), ok.as_str()]).expect("exclusion loads");
        let allow = &defs
            .rows()
            .last()
            .unwrap()
            .ambient
            .as_ref()
            .unwrap()
            .biome_allow;
        assert!(!biome_allowed(allow, mod_api::biome::DESERT));
        assert!(biome_allowed(allow, mod_api::biome::PLAINS));
        // No filter = all biomes.
        let defs = parse_layers(&[&base(), with_filter("").as_str()]).expect("no filter");
        assert!(defs
            .rows()
            .last()
            .unwrap()
            .ambient
            .as_ref()
            .unwrap()
            .biome_allow
            .is_none());
        // Unknown names and double declarations fail the load.
        for (bad, why) in [
            (
                with_filter(r#", "biomes": ["nope_biome"]"#),
                "unknown biome",
            ),
            (
                with_filter(r#", "biomes": ["desert"], "exclude_biomes": ["plains"]"#),
                "both list kinds",
            ),
        ] {
            assert!(
                parse_layers(&[&base(), bad.as_str()]).is_err(),
                "{why} must fail the load"
            );
        }
    }

    #[test]
    fn missing_engine_row_is_a_load_error() {
        assert!(parse_layers(&[r#"{"emitters": []}"#]).is_err());
    }

    #[test]
    fn ambient_bundles_validate_and_resolve_hit_bursts() {
        let ambient = |hit: &str, radius: &str| {
            format!(
                r#"{{"emitters": [{{"emitter": "mymod:rainfall", "ambient": {{
                    "count_per_intensity": 600, "max_count": 1500, "radius": {radius},
                    "height": [4, 20], "fall_speed": [16, 22], "drift_wind": 1.0,
                    "size": [0.03, 0.05], "stretch": 6.0, "alpha": [0.4, 0.7],
                    "color": [[0.55, 0.62, 0.75], [0.7, 0.78, 0.9]],
                    "hit": {hit} }} }}]}}"#
            )
        };

        // A valid ambient whose hit references the ENGINE water-splash burst.
        let ok = ambient(r#"{"burst": "petramond:water_splash"}"#, "24");
        let defs = parse_layers(&[&base(), ok.as_str()])
            .expect("ambient bundle loads")
            .rows();
        let d = defs.last().unwrap();
        let spec = d.ambient.as_ref().expect("ambient kind");
        assert!(matches!(&spec.hit, AmbientHit::Burst(k) if k == "petramond:water_splash"));
        assert!(d.rows.is_empty() && d.burst.is_none());

        // "die" is the default hit.
        let quiet = ambient(r#""die""#, "24");
        let defs = parse_layers(&[&base(), quiet.as_str()]).expect("die hit loads");
        assert_eq!(
            defs.rows().last().unwrap().ambient.as_ref().unwrap().hit,
            AmbientHit::Die
        );

        for (bad, why) in [
            (
                ambient(r#"{"burst": "mymod:nope"}"#, "24"),
                "an unknown hit bundle",
            ),
            (
                ambient(r#"{"burst": "petramond:torch_flame"}"#, "24"),
                "a hit bundle that is not a burst",
            ),
            (ambient(r#""die""#, "200"), "an out-of-range radius"),
            (
                r#"{"emitters": [{"emitter": "mymod:rainfall",
                    "ambient": {"count_per_intensity": 600, "max_count": 1500,
                        "radius": 24, "height": [4, 20], "fall_speed": [16, 22],
                        "size": [0.03, 0.05], "alpha": [0.4, 0.7],
                        "color": [[0.5, 0.5, 0.5], [0.7, 0.7, 0.7]]},
                    "burst": {"count_per_intensity": 3.0, "max_count": 24,
                        "up_speed": [1.0, 2.0], "radial_speed": [0.5, 1.5],
                        "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                        "color": [[0.1, 0.1, 0.5], [0.4, 0.8, 1.0]]} }]}"#
                    .to_owned(),
                "declaring both ambient and burst",
            ),
        ] {
            assert!(
                parse_layers(&[&base(), bad.as_str()]).is_err(),
                "{why} must fail the load"
            );
        }
    }
}
