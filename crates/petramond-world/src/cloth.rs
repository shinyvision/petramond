//! Keyed cloth rows: the layered `cloth.json` catalog. A cloth is a sheet of fabric
//! a block row hangs off itself (`"cloth": "<key>"`), pinned along its leading
//! vertical edge at `anchor` and simulated on each client for presentation only.
//!
//! Cloth never touches the sim: no collision, no saved state, no wire traffic beyond
//! the block it belongs to. The engine ships no rows; packs add namespaced ones.

use serde::Deserialize;

use crate::tile::Tile;

const ENGINE_CLOTH_NAMES: &[&str] = &[];

/// Grid resolution is bounded so one cloth's per-frame solve stays cheap.
pub const MAX_SEGMENTS: u8 = 24;
/// A cloth reaches at most this far from its anchor; the client culls and pads by it.
pub const MAX_EXTENT: f32 = 4.0;

pub struct ClothDef {
    pub id: u8,
    pub key: &'static str,
    pub tile: Tile,
    /// The part of the tile the sheet wears, as 0..1 fractions `[u0, v0, u1, v1]`;
    /// authored in texels so a non-square sheet keeps square pixels.
    pub uv: [f32; 4],
    /// Width away from the pinned edge, height down from the anchor, in blocks.
    pub size: [f32; 2],
    /// Quads across and down.
    pub segments: [u8; 2],
    /// Top of the pinned edge in the owning cell's 0..1 space.
    pub anchor: [f32; 3],
    /// Fraction of each constraint's error removed per pass, 0..=1.
    pub stiffness: f32,
    /// Velocity kept per step, 0..=1.
    pub damping: f32,
    /// How strongly the fabric catches the wind.
    pub wind: f32,
    pub gravity: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCloth {
    cloth: String,
    tile: String,
    #[serde(default = "full_uv")]
    uv: [f32; 4],
    size: [f32; 2],
    segments: [u8; 2],
    anchor: [f32; 3],
    #[serde(default = "default_stiffness")]
    stiffness: f32,
    #[serde(default = "default_damping")]
    damping: f32,
    #[serde(default = "one")]
    wind: f32,
    #[serde(default = "one")]
    gravity: f32,
}

fn full_uv() -> [f32; 4] {
    [0.0, 0.0, 16.0, 16.0]
}

fn default_stiffness() -> f32 {
    0.9
}

fn default_damping() -> f32 {
    0.985
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize)]
struct RawFile {
    cloths: Vec<RawCloth>,
}

pub fn by_key(key: &str) -> Option<&'static ClothDef> {
    catalog().id(key).map(|id| &catalog().rows()[id as usize])
}

pub fn def(id: u8) -> Option<&'static ClothDef> {
    catalog().rows().get(id as usize)
}

fn catalog() -> &'static crate::registry::Catalog<ClothDef> {
    CATALOG.current()
}

pub(crate) static CATALOG: crate::content::Slot<crate::registry::Catalog<ClothDef>> =
    crate::content::Slot::new(
        crate::content::stage::CLOTH,
        &[crate::content::stage::TILES],
        load,
    );

fn load(
    reg: &crate::content::ContentRegistry,
) -> Result<crate::registry::Catalog<ClothDef>, String> {
    crate::registry::read_catalog(reg.packs(), "cloth.json", "cloth", parse_layers)
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<ClothDef>, String> {
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.cloths),
        |r| &r.cloth,
        ENGINE_CLOTH_NAMES,
        "cloth",
        |r, id, names| {
            let err = |what: &str| format!("cloth '{}': {what}", r.cloth);
            let tile = Tile::from_name(&r.tile)
                .ok_or_else(|| err(&format!("unknown tile '{}'", r.tile)))?;
            if r.size
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0 || *v > MAX_EXTENT)
            {
                return Err(err(&format!("size must be in (0, {MAX_EXTENT}]")));
            }
            if r.segments.iter().any(|s| *s == 0 || *s > MAX_SEGMENTS) {
                return Err(err(&format!("segments must be in 1..={MAX_SEGMENTS}")));
            }
            let [u0, v0, u1, v1] = r.uv;
            if r.uv
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=16.0).contains(v))
                || u0 >= u1
                || v0 >= v1
            {
                return Err(err("uv must be a texel rect inside 0..=16 with min < max"));
            }
            if r.anchor
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(err("anchor must lie in the cell, 0..=1"));
            }
            for (label, v) in [("stiffness", r.stiffness), ("damping", r.damping)] {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err(err(&format!("{label} must be in 0..=1")));
                }
            }
            for (label, v) in [("wind", r.wind), ("gravity", r.gravity)] {
                if !v.is_finite() || !(0.0..=8.0).contains(&v) {
                    return Err(err(&format!("{label} must be in 0..=8")));
                }
            }
            Ok(ClothDef {
                id: id as u8,
                key: names.name(id).expect("id resolved from this table"),
                tile,
                uv: r.uv.map(|t| t / 16.0),
                size: r.size,
                segments: r.segments,
                anchor: r.anchor,
                stiffness: r.stiffness,
                damping: r.damping,
                wind: r.wind,
                gravity: r.gravity,
            })
        },
    )
}
