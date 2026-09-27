use serde::Deserialize;

pub const DEFAULT_ATTENUATION_DISTANCE: f32 = 32.0;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sound(pub u8);

#[allow(non_upper_case_globals)]
impl Sound {
    pub const WoodPunch: Sound = Sound(0);
    pub const WoodPlace: Sound = Sound(1);
    pub const WoodBreak: Sound = Sound(2);
    pub const ItemPickup: Sound = Sound(3);
    pub const DoorOpen: Sound = Sound(4);
    pub const DoorClose: Sound = Sound(5);
    pub const ChestOpen: Sound = Sound(6);
    pub const ChestClose: Sound = Sound(7);
    pub const StonePunch: Sound = Sound(8);
    pub const StoneBreak: Sound = Sound(9);
    pub const StonePlace: Sound = Sound(10);
    pub const DirtPunch: Sound = Sound(11);
    pub const DirtBreak: Sound = Sound(12);
    pub const DirtPlace: Sound = Sound(13);
    pub const PlayerHurt: Sound = Sound(14);
    pub const UiClick: Sound = Sound(15);
    pub const SheepIdle: Sound = Sound(16);
    pub const SheepHurt: Sound = Sound(17);
    pub const GlassPunch: Sound = Sound(20);
    pub const GlassBreak: Sound = Sound(21);
    pub const GlassPlace: Sound = Sound(22);
    pub const SandPunch: Sound = Sound(23);
    pub const SandBreak: Sound = Sound(24);
    pub const SandPlace: Sound = Sound(25);
    pub const LeafPunch: Sound = Sound(26);
    pub const LeafBreak: Sound = Sound(27);
    pub const LeafPlace: Sound = Sound(28);
    pub const WoodStep: Sound = Sound(29);
    pub const StoneStep: Sound = Sound(30);
    pub const DirtStep: Sound = Sound(31);
    pub const SandStep: Sound = Sound(32);
    pub const GlassStep: Sound = Sound(33);
    pub const LeafStep: Sound = Sound(34);
    pub const SnowPunch: Sound = Sound(35);
    pub const SnowBreak: Sound = Sound(36);
    pub const SnowPlace: Sound = Sound(37);
    pub const SnowStep: Sound = Sound(38);
}

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
    "petramond:sheep_idle",
    "petramond:sheep_hurt",
    "petramond:water_splash_small",
    "petramond:water_splash_big",
    "petramond:glass_punch",
    "petramond:glass_break",
    "petramond:glass_place",
    "petramond:sand_punch",
    "petramond:sand_break",
    "petramond:sand_place",
    "petramond:leaf_punch",
    "petramond:leaf_break",
    "petramond:leaf_place",
    "petramond:wood_step",
    "petramond:stone_step",
    "petramond:dirt_step",
    "petramond:sand_step",
    "petramond:glass_step",
    "petramond:leaf_step",
    "petramond:snow_punch",
    "petramond:snow_break",
    "petramond:snow_place",
    "petramond:snow_step",
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
    #[inline]
    pub fn def(self) -> &'static SoundDef {
        &defs()[self.0 as usize]
    }

    #[inline]
    pub fn distance_gain(self, distance: f32) -> f32 {
        distance_gain(distance, self.def().attenuation_distance)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundCategory {
    Block,
    Mob,
    Ui,
    Music,
}

#[allow(dead_code)]
pub struct SoundDef {
    pub sound: Sound,
    pub name: &'static str,
    pub variants: &'static [&'static str],
    pub gain: f32,
    pub pitch_variation: f32,
    pub pitch: f32,
    pub attenuation_distance: f32,
    pub category: SoundCategory,
    pub looped: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSoundDef {
    sound: String,
    variants: Vec<String>,
    gain: f64,
    pitch_variation: f64,
    #[serde(default = "default_pitch")]
    pitch: f64,
    #[serde(default = "default_attenuation_distance")]
    attenuation_distance: f64,
    category: SoundCategory,
    #[serde(default, rename = "loop")]
    looped: bool,
}

fn default_pitch() -> f64 {
    1.0
}

fn default_attenuation_distance() -> f64 {
    DEFAULT_ATTENUATION_DISTANCE as f64
}

#[derive(Deserialize)]
struct RawFile {
    sounds: Vec<RawSoundDef>,
}

pub fn by_name(name: &str) -> Option<Sound> {
    catalog().id(name).map(|id| Sound(id as u8))
}

pub fn defs() -> &'static [SoundDef] {
    catalog().rows()
}

pub(crate) static CATALOG: crate::content::Slot<crate::registry::Catalog<SoundDef>> =
    crate::content::Slot::new(crate::content::stage::SOUNDS, &[], load);

fn load(
    reg: &crate::content::ContentRegistry,
) -> Result<crate::registry::Catalog<SoundDef>, String> {
    crate::registry::read_catalog(reg.packs(), "sounds.json", "sound", parse_layers)
}

fn catalog() -> &'static crate::registry::Catalog<SoundDef> {
    CATALOG.current()
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<SoundDef>, String> {
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.sounds),
        |r| &r.sound,
        ENGINE_SOUND_NAMES,
        "sound",
        |r, id, names| {
            if !(r.attenuation_distance.is_finite() && r.attenuation_distance > 0.0) {
                return Err(format!(
                    "sound '{}': attenuation_distance must be finite and > 0",
                    r.sound
                ));
            }
            if !(r.pitch.is_finite() && r.pitch > 0.0) {
                return Err(format!("sound '{}': pitch must be finite and > 0", r.sound));
            }
            let variants: Vec<&'static str> = r
                .variants
                .into_iter()
                .map(|v| &*Box::leak(v.into_boxed_str()))
                .collect();
            Ok(SoundDef {
                sound: Sound(id as u8),
                name: names.name(id).expect("id resolved from this table"),
                variants: Box::leak(variants.into_boxed_slice()),
                gain: r.gain as f32,
                pitch_variation: r.pitch_variation as f32,
                pitch: r.pitch as f32,
                attenuation_distance: r.attenuation_distance as f32,
                category: r.category,
                looped: r.looped,
            })
        },
    )
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

    #[test]
    fn shipped_sounds_json_loads_fully() {
        let (text, path) =
            crate::assets::read_base_text("sounds.json").expect("assets/sounds.json must ship");
        let table = parse_layers(&[&text])
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .rows();
        assert_eq!(table.len(), ENGINE_SOUND_NAMES.len());
        for name in ENGINE_SOUND_NAMES {
            let def = table
                .iter()
                .find(|d| d.name == *name)
                .unwrap_or_else(|| panic!("engine sound '{name}' has a row"));
            assert_eq!(
                table[def.sound.0 as usize].name, *name,
                "name → id → def → name round-trips"
            );
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
        let table = parse_layers(&[&base, layer])
            .expect("layered table loads")
            .rows();
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
