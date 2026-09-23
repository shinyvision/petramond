//! The music catalog: a stable [`MusicTrack`] id per soundtrack piece, mapped to
//! its clip file and playback gain.
//!
//! Music is its own catalog rather than a `music`-category row of
//! `sounds.json`, for one structural reason: a sound is a short clip DECODED
//! INTO MEMORY at startup (see the playback engine), and a three-minute track
//! decoded to PCM is tens of megabytes — a handful of them would dwarf the
//! rest of the client's resident audio. A music row is therefore streamed from
//! its file at play time, and it also carries none of a sound row's shape (no
//! interchangeable variants, no pitch jitter, no positional reach, no
//! category — a music row IS the music category).
//!
//! The rows live in `assets/music.json`, a layered catalog like `sounds.json`:
//! add an engine track by adding a const + name here and a row + asset there;
//! a pack overrides an engine row by bare name or ADDS a track with a
//! namespaced (`mod_id:name`) key (see [`crate::registry`] for the shared
//! rules). Because the scheduler picks uniformly over the whole loaded table,
//! a pack that adds tracks joins the rotation with no engine change.

use std::sync::LazyLock;

use serde::Deserialize;

/// A soundtrack piece, identified by its opaque runtime id (the row index in
/// the loaded table). Engine tracks own the low ids in the frozen const order
/// below; pack tracks register after them.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MusicTrack(pub u8);

/// Engine track consts, in frozen id order.
#[allow(non_upper_case_globals)]
impl MusicTrack {
    pub const Firefly: MusicTrack = MusicTrack(0);
    pub const Footsteps: MusicTrack = MusicTrack(1);
    pub const Journey: MusicTrack = MusicTrack(2);
    pub const LittleMe: MusicTrack = MusicTrack(3);
    pub const Lullaby: MusicTrack = MusicTrack(4);
}

/// Engine track names in frozen id order (`ENGINE_MUSIC_NAMES[id]` names
/// `MusicTrack(id)`); the completeness oracle `music.json` is validated
/// against.
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
    /// This track's definition row.
    #[inline]
    pub fn def(self) -> &'static MusicDef {
        &defs()[self.0 as usize]
    }
}

/// One row of the music table. Playback fields are read only by the
/// `playback`-feature engine; the featureless (headless-server) build keeps
/// the table so the catalog still validates at startup.
#[allow(dead_code)]
pub struct MusicDef {
    pub track: MusicTrack,
    /// The row's registry name (`"petramond:firefly"`, `"mod_id:theme"`).
    pub name: &'static str,
    /// The track's source clip (OGG/Vorbis), as an asset-relative path
    /// (`music/...`) resolved through [`crate::assets`] — so a pack can
    /// replace a track by shipping the same path. Unlike a sound clip it is
    /// read and decoded when the track PLAYS, never at startup.
    pub file: &'static str,
    /// Base linear gain on top of the master/music mixer volumes (`1.0` = as
    /// mastered). The seam for trimming one track that sits louder than the
    /// rest, without re-encoding the asset.
    pub gain: f32,
}

/// One music row as written in `music.json`.
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

/// The runtime [`MusicTrack`] registered under `name`, or `None` when no such
/// row is loaded.
pub fn by_name(name: &str) -> Option<MusicTrack> {
    catalog().id(name).map(|id| MusicTrack(id as u8))
}

/// The loaded music table, id-ordered (`defs()[track.0]`). Loads exactly once;
/// a missing or inconsistent `music.json` fails loudly at startup.
pub fn defs() -> &'static [MusicDef] {
    catalog().rows()
}

fn catalog() -> &'static crate::registry::Catalog<MusicDef> {
    static TABLE: LazyLock<crate::registry::Catalog<MusicDef>> =
        LazyLock::new(|| crate::registry::read_catalog("music.json", "music track", parse_layers));
    &TABLE
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<MusicDef>, String> {
    crate::registry::load_catalog(
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

    /// The shipped `assets/music.json` must cover the engine track set — the
    /// startup gate, surfaced as a test — and every row's clip must actually
    /// be there. A track whose file is missing is silent at the exact moment
    /// it is meant to play, minutes into a session, which is the kind of gap
    /// a startup gate should catch instead.
    #[test]
    fn shipped_music_json_loads_fully_and_every_clip_resolves() {
        let (text, path) =
            crate::assets::read_base_text("music.json").expect("assets/music.json must ship");
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
                crate::assets::read_bytes(def.file).is_some(),
                "track '{name}' clip '{}' is missing",
                def.file
            );
        }
    }

    #[test]
    fn pack_layers_override_by_name_and_add_namespaced_tracks() {
        let (base, _) =
            crate::assets::read_base_text("music.json").expect("assets/music.json must ship");
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
