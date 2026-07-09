//! The sound-asset table: a stable [`Sound`] id per sound effect, mapped to its
//! clip files and default playback parameters.
//!
//! The rows live in `assets/sounds.json` (a layered catalog like `blocks.json`):
//! add an engine sound by adding a const + name here and a row + asset there; a
//! mod pack overrides an engine row by bare name or ADDS a sound with a
//! namespaced (`mod_id:name`) key, which registers a fresh id in load order
//! (see [`crate::registry`] for the shared rules).

use std::sync::LazyLock;

use serde::Deserialize;

/// Default distance, in blocks, where positional sounds fade to silence when a
/// row does not state its own reach.
pub(crate) const DEFAULT_ATTENUATION_DISTANCE: f32 = 32.0;

/// A sound effect the game can play, identified by its opaque runtime id (the
/// row index in the loaded table). Engine sounds own the low ids in the frozen
/// const order below; pack sounds register after them.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Sound(pub u8);

/// Engine sound consts, named like the enum variants they replaced.
#[allow(non_upper_case_globals)]
impl Sound {
    /// The wood "punch" — re-triggered while mining wood (see `crate::mining`).
    pub const WoodPunch: Sound = Sound(0);
    /// Placing a wood block.
    pub const WoodPlace: Sound = Sound(1);
    /// Breaking / destroying a wood block.
    pub const WoodBreak: Sound = Sound(2);
    /// Picking a dropped item up into the inventory — a global gameplay sound,
    /// not a block sound.
    pub const ItemPickup: Sound = Sound(3);
    /// A door swung open (its `open` bit just flipped to true).
    pub const DoorOpen: Sound = Sound(4);
    /// A door swung shut (its `open` bit just flipped to false).
    pub const DoorClose: Sound = Sound(5);
    /// A chest's lid is swinging open (its GUI was just opened).
    pub const ChestOpen: Sound = Sound(6);
    /// A chest's lid is dropping shut (its GUI was just closed).
    pub const ChestClose: Sound = Sound(7);
    /// The stone "punch" — re-triggered while mining stone (and ore, which
    /// shares the stone set). See [`crate::block::sounds::STONE`].
    pub const StonePunch: Sound = Sound(8);
    /// A stone block finished breaking / was destroyed.
    pub const StoneBreak: Sound = Sound(9);
    /// A stone block was placed into the world.
    pub const StonePlace: Sound = Sound(10);
    /// The dirt "punch" — re-triggered while mining dirt, grass, gravel, and
    /// other dirt-likes. See [`crate::block::sounds::DIRT`].
    pub const DirtPunch: Sound = Sound(11);
    /// A dirt block finished breaking / was destroyed.
    pub const DirtBreak: Sound = Sound(12);
    /// A dirt block was placed into the world.
    pub const DirtPlace: Sound = Sound(13);
    /// The player took damage (any source) — player feedback, non-positional.
    pub const PlayerHurt: Sound = Sound(14);
    /// A shell/UI button or toggle was activated.
    pub const UiClick: Sound = Sound(15);
}

/// Engine sound names in frozen id order (`ENGINE_SOUND_NAMES[id]` names
/// `Sound(id)`); the completeness oracle `sounds.json` is validated against.
const ENGINE_SOUND_NAMES: &[&str] = &[
    "petramond:wood_punch",
    "petramond:wood_place",
    "petramond:wood_break",
    "petramond:item_pickup",
    "petramond:door_open",
    "petramond:door_close",
    "petramond:chest_open",
    "petramond:chest_close",
    "petramond:stone_punch",
    "petramond:stone_break",
    "petramond:stone_place",
    "petramond:dirt_punch",
    "petramond:dirt_break",
    "petramond:dirt_place",
    "petramond:player_hurt",
    "petramond:ui_click",
];

impl std::fmt::Debug for Sound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_SOUND_NAMES.get(self.0 as usize) {
            Some(name) => write!(f, "Sound({name})"),
            None => write!(f, "Sound(#{})", self.0),
        }
    }
}

impl Sound {
    /// This sound's definition row.
    #[inline]
    pub(crate) fn def(self) -> &'static SoundDef {
        &defs()[self.0 as usize]
    }

    /// Distance gain for this sound's row-owned travel distance. The curve fades
    /// slowly near the listener and reaches silence at `attenuation_distance`.
    #[inline]
    pub(crate) fn distance_gain(self, distance: f32) -> f32 {
        distance_gain(distance, self.def().attenuation_distance)
    }
}

/// The broad mixing group a sound belongs to. Effective gain is
/// `master × category × per-sound`; all categories are full volume today, so this
/// is the hook for a future per-category (e.g. options-menu) volume control.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundCategory {
    /// Block interaction — mining, breaking, placing, footsteps.
    Block,
    /// Creature and entity presentation — idle calls, hurt barks, deaths.
    Mob,
    /// UI / interface & player feedback — menu clicks, item pickup, etc.
    Ui,
}

/// One row of the sound table: a sound's clip files + default playback
/// parameters. Clips are read through the asset roots (so a mod pack can
/// override one by shipping the same relative path) and decoded once at
/// startup (see [`crate::audio::Audio`]); this is just the static description.
/// Playback fields are read only by the `audio`-feature engine; the featureless
/// (headless-server) build keeps the table for names/net tables alone.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(crate) struct SoundDef {
    pub sound: Sound,
    /// The row's registry name (`"petramond:wood_punch"`, `"mod_id:zap"`) — the key mod
    /// `EmitSound` HostCalls resolve through [`by_name`].
    pub name: &'static str,
    /// One or more interchangeable source clips (OGG/Vorbis), as asset-relative
    /// paths (`sounds/...`) resolved through [`crate::assets`] at startup. A random
    /// variant is chosen each play — on top of the per-play pitch jitter — so a
    /// repeated sound never sounds identical. Order is irrelevant; add or remove
    /// clips freely.
    pub variants: &'static [&'static str],
    /// Base linear gain on top of the category/master gain (`1.0` = unit).
    pub gain: f32,
    /// Per-play pitch jitter as a ± fraction of unit playback speed: each play picks
    /// a random speed in `[1 - v, 1 + v]`, so a repeated sound never sounds
    /// identical. `0.0` = none. (Speed shifts pitch — rodio `Source::speed`.)
    pub pitch_variation: f32,
    /// Distance in blocks where positional playback fades to silence.
    pub attenuation_distance: f32,
    pub category: SoundCategory,
}

/// One sound row as written in `sounds.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSoundDef {
    sound: String,
    variants: Vec<String>,
    gain: f64,
    pitch_variation: f64,
    #[serde(default = "default_attenuation_distance")]
    attenuation_distance: f64,
    category: SoundCategory,
}

fn default_attenuation_distance() -> f64 {
    DEFAULT_ATTENUATION_DISTANCE as f64
}

#[derive(Deserialize)]
struct RawFile {
    sounds: Vec<RawSoundDef>,
}

/// The runtime [`Sound`] registered under `name` (engine `petramond:*` and pack
/// `mod_id:name` keys alike), or `None` when no such row is loaded.
pub(crate) fn by_name(name: &str) -> Option<Sound> {
    defs().iter().find(|d| d.name == name).map(|d| d.sound)
}

/// The loaded sound table, id-ordered (`defs()[sound.0]`). Loads exactly once;
/// a missing or inconsistent `sounds.json` fails loudly at startup.
pub(crate) fn defs() -> &'static [SoundDef] {
    static TABLE: LazyLock<&'static [SoundDef]> = LazyLock::new(|| {
        let layers = crate::assets::read_layers("sounds.json");
        if layers.is_empty() {
            panic!(
                "sounds.json not found (searched {:?}); the game cannot run without its sound table",
                crate::assets::candidate_paths("sounds.json")
            );
        }
        let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
        Box::leak(
            parse_layers(&texts)
                .unwrap_or_else(|e| panic!("sounds.json: {e}"))
                .into_boxed_slice(),
        )
    });
    &TABLE
}

fn parse_layers(texts: &[&str]) -> Result<Vec<SoundDef>, String> {
    // Merge layers by sound key, then assign ids: engine names hold their
    // frozen ids, namespaced keys register after them (bare unknowns error) —
    // the same contract as the block/item catalogs.
    let mut merged: Vec<RawSoundDef> = Vec::new();
    let mut layer_keys: Vec<Vec<String>> = Vec::new();
    for (li, text) in texts.iter().enumerate() {
        let raw: RawFile =
            serde_json::from_str(text).map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        layer_keys.push(raw.sounds.iter().map(|r| r.sound.clone()).collect());
        for r in raw.sounds {
            match merged.iter_mut().find(|m| m.sound == r.sound) {
                Some(slot) => *slot = r,
                None => merged.push(r),
            }
        }
    }
    let names = crate::registry::NameTable::build(ENGINE_SOUND_NAMES, &layer_keys, "sound")?;
    let mut rows: Vec<Option<SoundDef>> = (0..names.len()).map(|_| None).collect();
    for r in merged {
        if !(r.attenuation_distance.is_finite() && r.attenuation_distance > 0.0) {
            return Err(format!(
                "sound '{}': attenuation_distance must be finite and > 0",
                r.sound
            ));
        }
        let id = names
            .id(&r.sound)
            .ok_or_else(|| format!("unregistered sound '{}'", r.sound))?;
        let variants: Vec<&'static str> = r
            .variants
            .into_iter()
            .map(|v| &*Box::leak(v.into_boxed_str()))
            .collect();
        rows[id as usize] = Some(SoundDef {
            sound: Sound(id),
            name: names.name(id).expect("id resolved from this table"),
            variants: Box::leak(variants.into_boxed_slice()),
            gain: r.gain as f32,
            pitch_variation: r.pitch_variation as f32,
            attenuation_distance: r.attenuation_distance as f32,
            category: r.category,
        });
    }
    rows.into_iter()
        .enumerate()
        .map(|(id, row)| {
            row.ok_or_else(|| {
                format!(
                    "missing row for sound '{}'",
                    names.name(id as u8).unwrap_or("?")
                )
            })
        })
        .collect()
}

fn distance_gain(distance: f32, attenuation_distance: f32) -> f32 {
    if !(distance.is_finite() && attenuation_distance.is_finite()) || attenuation_distance <= 0.0 {
        return 0.0;
    }
    let t = (distance.max(0.0) / attenuation_distance).clamp(0.0, 1.0);
    1.0 - t * t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `assets/sounds.json` must cover the engine sound set — the
    /// startup gate, surfaced as a test.
    #[test]
    fn shipped_sounds_json_loads_fully() {
        let (text, path) =
            crate::assets::read_base_text("sounds.json").expect("assets/sounds.json must ship");
        let table = parse_layers(&[&text]).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(table.len(), ENGINE_SOUND_NAMES.len());
        for (id, def) in table.iter().enumerate() {
            assert_eq!(def.sound, Sound(id as u8), "table is id-ordered");
        }
    }

    #[test]
    fn pack_layers_override_by_name_and_add_namespaced_sounds() {
        let (base, _) =
            crate::assets::read_base_text("sounds.json").expect("assets/sounds.json must ship");
        let layer = r#"{"sounds": [
            {"sound": "petramond:wood_punch", "variants": ["sounds/wood_punch_1.ogg"], "gain": 0.5, "pitch_variation": 0.0, "category": "block"},
            {"sound": "mymod:zap", "variants": ["sounds/zap.ogg"], "gain": 1.0, "pitch_variation": 0.1, "attenuation_distance": 48.0, "category": "ui"}
        ]}"#;
        let table = parse_layers(&[&base, layer]).expect("layered table loads");
        let engine = ENGINE_SOUND_NAMES.len();
        assert_eq!(table.len(), engine + 1, "the namespaced row registered");
        assert_eq!(
            table[Sound::WoodPunch.0 as usize].gain,
            0.5,
            "override applied"
        );
        assert_eq!(
            table[engine].variants,
            ["sounds/zap.ogg"],
            "dynamic row loaded"
        );
        assert_eq!(
            table[engine].attenuation_distance, 48.0,
            "pack rows can choose their positional reach"
        );
        assert_eq!(
            table[Sound::WoodPlace.0 as usize].attenuation_distance,
            DEFAULT_ATTENUATION_DISTANCE,
            "omitted reach uses the default"
        );
        // A NEW bare name is refused.
        let bare = r#"{"sounds": [{"sound": "zap", "variants": [], "gain": 1, "pitch_variation": 0, "category": "ui"}]}"#;
        let err = parse_layers(&[&base, bare])
            .err()
            .expect("bare additions refused");
        assert!(err.contains("zap") && err.contains("namespace"), "{err}");
    }

    #[test]
    fn distance_falloff_is_gradual_and_reaches_silence_at_the_row_distance() {
        assert_eq!(distance_gain(0.0, 32.0), 1.0);
        assert!(
            distance_gain(10.0, 32.0) > 0.85,
            "ten-block sounds should still be clearly audible"
        );
        assert_eq!(distance_gain(32.0, 32.0), 0.0);
        assert_eq!(distance_gain(64.0, 32.0), 0.0);
    }
}
