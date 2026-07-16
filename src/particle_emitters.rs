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

/// One loaded bundle (`defs()[id]`). Either LOOPING (`rows` non-empty: shown
/// continuously while attached to a block/mob) or a ONE-SHOT `burst` (spawned
/// once per `EmitterBurst` world event, simulated with gravity + collision).
pub struct EmitterBundle {
    /// The bundle's session id (its row index).
    pub id: u8,
    /// The registry key (`"petramond:burn_light"`, `"mod_id:sparkle"`).
    pub key: &'static str,
    /// Optional multiply body tint shown while attached to a mob (RGB
    /// `0..=1`). Ignored by block references.
    pub tint: Option<[f32; 3]>,
    /// The looping particle rows, all shown together while the bundle is
    /// active. Empty for a burst bundle.
    pub rows: &'static [ParticleEmitter],
    /// One-shot burst parameters. `Some` exactly when `rows` is empty.
    pub burst: Option<BurstSpec>,
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

/// One bundle row as written in `particle_emitters.json`: exactly one of
/// `particles` (looping) or `burst` (one-shot).
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
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.emitters),
        |r| &r.emitter,
        ENGINE_EMITTER_NAMES,
        "emitter",
        |r, id, names| {
            match (&r.burst, r.particles.len()) {
                (None, 0) => {
                    return Err(format!(
                        "emitter '{}': needs either particles (looping) or burst (one-shot)",
                        r.emitter
                    ))
                }
                (Some(_), 1..) => {
                    return Err(format!(
                        "emitter '{}': declares both particles and burst — pick one",
                        r.emitter
                    ))
                }
                (None, n) if n > MAX_BUNDLE_ROWS => {
                    return Err(format!(
                        "emitter '{}': 1..={MAX_BUNDLE_ROWS} particle rows per bundle, got {n}",
                        r.emitter
                    ))
                }
                _ => {}
            }
            if let Some(burst) = &r.burst {
                validate_burst(&r.emitter, burst)?;
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
            })
        },
    )
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
            assert!(
                d.rows.is_empty() == d.burst.is_some(),
                "every bundle is looping XOR burst"
            );
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
    fn missing_engine_row_is_a_load_error() {
        assert!(parse_layers(&[r#"{"emitters": []}"#]).is_err());
    }
}
