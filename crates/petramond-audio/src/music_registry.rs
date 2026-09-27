use petramond_world::content::{ContentRegistry, Slot};
use petramond_world::registry;
use serde::Deserialize;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MusicTrack(pub u8);

#[allow(non_upper_case_globals)]
impl MusicTrack {
    pub const Firefly: MusicTrack = MusicTrack(0);
    pub const Footsteps: MusicTrack = MusicTrack(1);
    pub const Journey: MusicTrack = MusicTrack(2);
    pub const LittleMe: MusicTrack = MusicTrack(3);
    pub const Lullaby: MusicTrack = MusicTrack(4);
}

const ENGINE_MUSIC_NAMES: &[&str] = &[
    "petramond:firefly",
    "petramond:footsteps",
    "petramond:journey",
    "petramond:little_me",
    "petramond:lullaby",
];

impl std::fmt::Debug for MusicTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_MUSIC_NAMES.get(self.0 as usize) {
            Some(name) => write!(f, "MusicTrack({name})"),
            None => write!(f, "MusicTrack(#{})", self.0),
        }
    }
}

impl MusicTrack {
    #[inline]
    pub fn def(self) -> &'static MusicDef {
        &defs()[self.0 as usize]
    }
}

#[allow(dead_code)]
pub struct MusicDef {
    pub track: MusicTrack,
    pub name: &'static str,
    pub file: &'static str,
    pub gain: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMusicDef {
    track: String,
    file: String,
    #[serde(default = "default_gain")]
    gain: f64,
}

fn default_gain() -> f64 {
    1.0
}

#[derive(Deserialize)]
struct RawFile {
    tracks: Vec<RawMusicDef>,
}

pub fn by_name(name: &str) -> Option<MusicTrack> {
    catalog().id(name).map(|id| MusicTrack(id as u8))
}

pub fn defs() -> &'static [MusicDef] {
    catalog().rows()
}

pub static CATALOG: Slot<registry::Catalog<MusicDef>> = Slot::new("music.json", &[], load);

fn load(reg: &ContentRegistry) -> Result<registry::Catalog<MusicDef>, String> {
    registry::read_catalog(reg.packs(), "music.json", "music track", parse_layers)
}

fn catalog() -> &'static registry::Catalog<MusicDef> {
    CATALOG.current()
}

fn parse_layers(texts: &[&str]) -> Result<registry::Catalog<MusicDef>, String> {
    registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.tracks),
        |r| &r.track,
        ENGINE_MUSIC_NAMES,
        "music track",
        |r, id, names| {
            if !(r.gain.is_finite() && r.gain >= 0.0) {
                return Err(format!(
                    "music track '{}': gain must be finite and >= 0",
                    r.track
                ));
            }
            Ok(MusicDef {
                track: MusicTrack(id as u8),
                name: names.name(id).expect("id resolved from this table"),
                file: Box::leak(r.file.into_boxed_str()),
                gain: r.gain as f32,
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::assets;

    #[test]
    fn shipped_music_json_loads_fully_and_every_clip_resolves() {
        let (text, path) =
            assets::read_base_text("music.json").expect("assets/music.json must ship");
        let table = parse_layers(&[&text])
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .rows();
        assert_eq!(table.len(), ENGINE_MUSIC_NAMES.len());
        for name in ENGINE_MUSIC_NAMES {
            let def = table
                .iter()
                .find(|d| d.name == *name)
                .unwrap_or_else(|| panic!("engine track '{name}' has a row"));
            assert_eq!(
                table[def.track.0 as usize].name, *name,
                "name → id → def → name round-trips"
            );
            assert!(
                assets::read_bytes(def.file).is_some(),
                "track '{name}' clip '{}' is missing",
                def.file
            );
        }
    }

    #[test]
    fn pack_layers_override_by_name_and_add_namespaced_tracks() {
        let (base, _) = assets::read_base_text("music.json").expect("assets/music.json must ship");
        let layer = r#"{"tracks": [
            {"track": "petramond:firefly", "file": "music/firefly.ogg", "gain": 0.5},
            {"track": "mymod:theme", "file": "music/theme.ogg"}
        ]}"#;
        let table = parse_layers(&[&base, layer])
            .expect("layered table loads")
            .rows();
        let engine = ENGINE_MUSIC_NAMES.len();
        assert_eq!(table.len(), engine + 1, "the namespaced row registered");
        assert_eq!(
            table[MusicTrack::Firefly.0 as usize].gain,
            0.5,
            "the layer replaced the engine row in place"
        );
        assert_eq!(
            table[engine].name, "mymod:theme",
            "a pack track registers past the engine ids"
        );
        assert_eq!(table[engine].gain, 1.0, "an unstated gain defaults to unit");
    }
}
