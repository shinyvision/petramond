//! Tile IDENTITY registry: names, ids, tint classes, animation frame counts.
//!
//! Composed from the layered `assets/textures/atlas.json` manifests. This is
//! the half of the atlas every layer may depend on — block/item data rows
//! resolve tile names here, and a headless server assigns the exact same ids
//! without ever decoding a texel. Pixel composition (the composed RGBA atlas,
//! mips, UV rects, alpha bounds) lives in `atlas`, which consumes
//! [`cells`] on the client.
//!
//! Animated strips expand into one tile id per frame, so id assignment needs
//! each strip's frame count — read from the PNG HEADER (dimensions only),
//! never the pixels.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

use serde::Deserialize;

/// How many tiles the id space can address — the ONE definition the manifest
/// loader's cap, the packed chunk vertex's tile-id field, and the shader
/// uv-rect table all derive from, so the three cannot drift.
pub const MAX_TILES: usize = 2048;

/// One 16×16 cell of the atlas, identified by its load-time index. Stable
/// WITHIN a run (data files reference tiles by name; numeric ids are assigned
/// at load and never persisted).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tile(u16);

/// Which biome colour a tile tints with (see `mesh::tint` for the in-world
/// blend and `render::foliage_tint` for the fixed out-of-world colour).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileTint {
    Grass,
    Foliage,
    Water,
    /// Fixed albedo multiply, independent of the column biome.
    Fixed([u8; 3]),
}

/// How terrain picks one of a tile row's static `variants` (see
/// [`Tile::face_variation`] and the shader's `block_variant_layer`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariationSelect {
    /// Per exposed face, chosen by the mesher: each face of a cell may show a
    /// different alternative. Right for cutout foliage, whose faces never
    /// greedy-merge anyway.
    Face,
    /// Per world cell, chosen in the fragment shader from the texel's owning
    /// cell: all six faces agree, and greedy merging is unaffected because the
    /// mesh keeps the base tile id.
    Cell,
}

/// Multiply-xor mixer constants of [`spatial_hash`]; the render pipeline
/// emits the same function into WGSL from these, so both selectors are one
/// algorithm with one definition.
pub const SPATIAL_HASH_AXIS_MULTIPLIERS: [u32; 3] = [0x8da6_b343, 0xd816_3841, 0xcb1a_b31f];
pub const SPATIAL_HASH_SALT_MULTIPLIER: u32 = 0x9e37_79b9;
pub const SPATIAL_HASH_MIX_MULTIPLIERS: [u32; 2] = [0x7feb_352d, 0x846c_a68b];

/// A stable pseudo-random word for a world cell (plus a small `salt`, e.g. a
/// face's normal code), identical wherever the cell is meshed or drawn.
#[inline]
pub fn spatial_hash(cell: [i32; 3], salt: u32) -> u32 {
    let [mx, my, mz] = SPATIAL_HASH_AXIS_MULTIPLIERS;
    let [x, y, z] = cell.map(|v| v as u32);
    let mut h = x.wrapping_mul(mx)
        ^ y.wrapping_mul(my)
        ^ z.wrapping_mul(mz)
        ^ salt.wrapping_mul(SPATIAL_HASH_SALT_MULTIPLIER);
    h ^= h >> 16;
    h = h.wrapping_mul(SPATIAL_HASH_MIX_MULTIPLIERS[0]);
    h ^= h >> 15;
    h = h.wrapping_mul(SPATIAL_HASH_MIX_MULTIPLIERS[1]);
    h ^ (h >> 16)
}

impl Tile {
    /// This tile's atlas index (also its texture-array layer).
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// This tile's index as the raw id the packed chunk vertex carries.
    #[inline]
    pub fn id(self) -> u16 {
        self.0
    }

    /// The tile's stable snake_case asset name (its manifest identity).
    #[inline]
    pub fn name(self) -> &'static str {
        data().names[self.index()]
    }

    /// Resolve a manifest name to its tile.
    pub fn from_name(name: &str) -> Option<Tile> {
        data().by_name.get(name).copied()
    }

    /// [`from_name`](Self::from_name) for a tile the caller knows must exist
    /// (engine code, tests); panics with the name if the manifest lost it.
    pub fn named(name: &str) -> Tile {
        Tile::from_name(name).unwrap_or_else(|| panic!("no tile named '{name}' in the atlas"))
    }

    /// Flipbook frame count for an animated tile (0 = static). The frames
    /// occupy this tile's index and the next `n-1` consecutive tiles, so the
    /// shader samples `base + frame` to animate.
    #[inline]
    pub fn anim_frames(self) -> u32 {
        data().cells[self.index()].anim_frames
    }

    /// How many consecutive static alternatives this tile heads, itself
    /// included (1 = no variants; alternatives and animation frames answer 1).
    #[inline]
    pub fn variation_count(self) -> usize {
        data().cells[self.index()].variation_count as usize
    }

    /// How terrain chooses among this tile's alternatives, from its manifest
    /// row; `None` = consumers address alternatives explicitly.
    #[inline]
    pub fn variation_select(self) -> Option<VariationSelect> {
        data().cells[self.index()].variation
    }

    /// The alternative `seed` selects, the base tile included. Alternatives
    /// share the base's tint and mip policy; a tile without alternatives
    /// answers itself.
    #[inline]
    pub fn variation(self, seed: u32) -> Tile {
        Tile(self.0 + (seed % data().cells[self.index()].variation_count as u32) as u16)
    }

    /// The alternative a FACE-selecting tile shows on the face of world cell
    /// `cell` that points along normal code `normal`; any other tile answers
    /// itself. The CPU half of [`VariationSelect`] — the shader owns the
    /// per-cell half over the same [`spatial_hash`].
    #[inline]
    pub fn face_variation(self, cell: [i32; 3], normal: u32) -> Tile {
        let meta = &data().cells[self.index()];
        match meta.variation {
            Some(VariationSelect::Face) => {
                Tile(self.0 + (spatial_hash(cell, normal) % meta.variation_count as u32) as u16)
            }
            _ => self,
        }
    }

    /// This tile's IN-WORLD biome-tint class (what the chunk mesher applies),
    /// from its manifest row. `None` = untinted.
    #[inline]
    pub fn world_tint(self) -> Option<TileTint> {
        data().cells[self.index()].world_tint
    }

    /// This tile's OUT-OF-WORLD tint class — icons, held/dropped items, and
    /// break particles, which have no biome context. Defaults to
    /// [`world_tint`](Self::world_tint); a manifest row overrides it with
    /// `icon_tint` when the two classifications differ (azalea leaves
    /// green in icons but keep their baked colour in the world).
    #[inline]
    pub fn icon_tint(self) -> Option<TileTint> {
        let c = &data().cells[self.index()];
        c.icon_tint.or(c.world_tint)
    }

    /// Number of tiles in the atlas.
    #[inline]
    pub fn count() -> usize {
        data().cells.len()
    }

    /// Every tile, in id order.
    pub fn all() -> impl Iterator<Item = Tile> {
        (0..Tile::count() as u16).map(Tile)
    }
}

/// Per-tile representative cartography colours, pixel-derived by the atlas
/// composer and installed once on the client (`atlas` calls
/// [`install_map_colors`] when it composes). Headless builds never install —
/// [`map_rgb`] then answers a neutral gray, and nothing headless draws maps.
static MAP_RGB: OnceLock<Vec<[u8; 3]>> = OnceLock::new();

/// Install the pixel-derived per-tile map colours (id order). First caller
/// wins; the composer only ever runs once.
pub fn install_map_colors(colors: Vec<[u8; 3]>) {
    let _ = MAP_RGB.set(colors);
}

/// Representative untinted top-down cartography colour for `tile`. Callers
/// apply the same biome tint as terrain. Neutral gray until the atlas
/// composer installs the real colours (headless: always).
pub fn map_rgb(tile: Tile) -> [u8; 3] {
    MAP_RGB
        .get()
        .and_then(|v| v.get(tile.index()))
        .copied()
        .unwrap_or([32, 32, 32])
}

/// Tiles the ENGINE itself references (the custom chest model, the grass-side
/// compositing, the break-overlay stages) — resolved once at registry load.
/// Content tiles flow through block/item data rows instead (a fluid's still and
/// flow strips are its row's `tiles` and `flow_tile`); a tile belongs here only
/// when engine CODE, not data, needs it.
pub struct EngineTiles {
    /// The grass-block side compositing set: an untinted `dirt` base with the
    /// biome-tinted grayscale `grass_side_overlay` on top, applied wherever the
    /// mesher (or the out-of-world item renderer) meets `grass_side`.
    pub grass_side: Tile,
    pub grass_side_overlay: Tile,
    pub dirt: Tile,
    /// The custom inset chest model's face set (see `render::chest_model`).
    pub chest_top: Tile,
    pub chest_front: Tile,
    pub chest_side: Tile,
    pub chest_lid_front: Tile,
    pub chest_lid_side: Tile,
    pub chest_inside: Tile,
    pub chest_latch: Tile,
    /// Break-progress crack overlays, stage 0 (first crack) to 9 (shattering).
    pub destroy_stages: [Tile; 10],
    /// The flat white the streak behind a fast-flying item samples (the
    /// renderer's `item_entity` trail) — light and taper are the geometry's.
    pub item_trail: Tile,
}

/// The engine-referenced tiles, resolved once at registry load.
#[inline]
pub fn engine() -> &'static EngineTiles {
    &data().engine
}

/// The game tick rate a flipbook's `frame_ticks` count in.
pub const TICKS_PER_SECOND: f32 = 20.0;

/// One tile id's source cell: which manifest file feeds it, and which frame of
/// that file when the row is an animated strip. The atlas pixel composer
/// consumes these; identity consumers never touch `file`/`frame`.
pub struct CellMeta {
    pub name: String,
    pub file: String,
    /// Frame index within `file` (0 for static tiles).
    pub frame: u32,
    /// On the BASE frame of an animated tile: total frames. 0 otherwise.
    pub anim_frames: u32,
    /// Game ticks each frame shows ([`TICKS_PER_SECOND`]` / frame_ticks`
    /// fps); fractional, so a row authored in `fps` runs at exactly that rate.
    pub frame_ticks: f32,
    pub interpolate: bool,
    /// Consecutive static alternatives, including this base. 1 on alternatives.
    pub variation_count: u16,
    /// How terrain selects among them; only the base cell of a row carries it.
    pub variation: Option<VariationSelect>,
    pub world_tint: Option<TileTint>,
    pub icon_tint: Option<TileTint>,
    /// Alpha-expand while downsampling mips, so distant cutout gaps fill with
    /// nearby colour instead of disappearing under the shader's alpha test.
    pub fill_cutout_mips: bool,
}

/// Every tile's source cell, in id order (the atlas composer's input).
pub fn cells() -> &'static [CellMeta] {
    &data().cells
}

#[derive(Deserialize)]
struct RawManifest {
    tiles: Vec<RawTile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTile {
    name: String,
    file: String,
    /// A vertical strip of square frames, expanded into consecutive tiles.
    #[serde(default)]
    anim: bool,
    /// Game ticks per frame. Exclusive with `fps`; absent both = 1 tick.
    #[serde(default)]
    frame_ticks: Option<f32>,
    /// Frames per second, for rates a tick count cannot spell exactly.
    #[serde(default)]
    fps: Option<f32>,
    #[serde(default)]
    interpolate: bool,
    #[serde(default)]
    tint: Option<TileTint>,
    #[serde(default)]
    icon_tint: Option<TileTint>,
    #[serde(default)]
    fill_cutout_mips: bool,
    /// Consecutive static alternatives, relative to `textures/`.
    #[serde(default)]
    variants: Vec<String>,
    /// How terrain chooses among `variants`; absent = addressed explicitly.
    #[serde(default)]
    variation: Option<VariationSelect>,
}

impl RawTile {
    /// Game ticks each frame shows, from whichever rate the row authors.
    fn frame_ticks(&self) -> Result<f32, String> {
        let positive = |v: f32| v.is_finite() && v > 0.0;
        match (self.frame_ticks, self.fps) {
            (Some(_), Some(_)) => Err(format!(
                "tile '{}' declares both frame_ticks and fps",
                self.name
            )),
            (Some(ticks), None) if positive(ticks) => Ok(ticks),
            (Some(_), None) => Err(format!("tile '{}' frame_ticks must be positive", self.name)),
            (None, Some(fps)) if positive(fps) => Ok(TICKS_PER_SECOND / fps),
            (None, Some(_)) => Err(format!("tile '{}' fps must be positive", self.name)),
            (None, None) => Ok(1.0),
        }
    }
}

struct TileData {
    cells: Vec<CellMeta>,
    names: Vec<&'static str>,
    by_name: HashMap<&'static str, Tile>,
    engine: EngineTiles,
}

static TILES: LazyLock<TileData> = LazyLock::new(|| {
    let layers = crate::assets::read_layers("textures/atlas.json");
    if layers.is_empty() {
        panic!(
            "textures/atlas.json not found (searched {:?}); the game cannot run without its texture atlas",
            crate::assets::candidate_paths("textures/atlas.json")
        );
    }
    for (_, path) in &layers {
        log::info!("atlas manifest layer: {}", path.display());
    }
    let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
    build(&texts).unwrap_or_else(|e| panic!("textures/atlas.json: {e}"))
});

#[cfg(test)]
mod variation_tests;

#[inline]
fn data() -> &'static TileData {
    &TILES
}

/// A manifest PNG's dimensions through the asset roots — header decode only,
/// never the pixels (this is what lets a headless build assign tile ids).
fn image_dimensions(file: &str) -> Result<(u32, u32), String> {
    let rel = format!("textures/{file}");
    let (bytes, _) = crate::assets::read_bytes(&rel)
        .ok_or_else(|| format!("missing texture '{rel}' (searched the asset roots)"))?;
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("failed to read '{rel}': {e}"))?
        .into_dimensions()
        .map_err(|e| format!("failed to decode '{rel}' header: {e}"))
}

fn build(manifests: &[&str]) -> Result<TileData, String> {
    // Merge manifest layers by tile name: a later layer's row REPLACES the
    // earlier one (keeping its position, so replacement never renumbers ids
    // within a run); unknown names APPEND as new tiles. A pack can thus both
    // reskin a tile (with its own PNG resolved point-file-first anyway) and
    // add brand-new tiles for its item sprites.
    let mut rows: Vec<RawTile> = Vec::new();
    for (li, manifest) in manifests.iter().enumerate() {
        let raw: RawManifest = serde_json::from_str(manifest)
            .map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        for t in raw.tiles {
            match rows.iter_mut().find(|r| r.name == t.name) {
                Some(slot) => *slot = t,
                None => rows.push(t),
            }
        }
    }

    // Expand the manifest into the flat ordered cell list (animated strips
    // contribute one cell per frame).
    let mut cells: Vec<CellMeta> = Vec::new();
    for t in &rows {
        let frame_ticks = t.frame_ticks()?;
        if t.variation.is_some() && t.variants.is_empty() {
            return Err(format!(
                "tile '{}' declares a variation selector without variants",
                t.name
            ));
        }
        if t.anim && !t.variants.is_empty() {
            return Err(format!(
                "tile '{}' cannot combine animation and static variants",
                t.name
            ));
        }
        if t.variants.len() >= MAX_TILES {
            return Err(format!("tile '{}' has too many variants", t.name));
        }
        if !t.anim {
            for (i, file) in std::iter::once(&t.file).chain(&t.variants).enumerate() {
                cells.push(CellMeta {
                    name: if i == 0 {
                        t.name.clone()
                    } else {
                        format!("{}_variant_{i}", t.name)
                    },
                    file: file.clone(),
                    frame: 0,
                    anim_frames: 0,
                    frame_ticks: 1.0,
                    interpolate: false,
                    variation_count: if i == 0 {
                        (t.variants.len() + 1) as u16
                    } else {
                        1
                    },
                    variation: t.variation.filter(|_| i == 0),
                    world_tint: t.tint,
                    icon_tint: t.icon_tint,
                    fill_cutout_mips: t.fill_cutout_mips,
                });
            }
            continue;
        }
        let (sw, sh) = image_dimensions(&t.file)?;
        if sw == 0 || sh == 0 || sh % sw != 0 {
            return Err(format!(
                "animated texture 'textures/{}' must be a vertical strip of square frames, got {sw}x{sh}",
                t.file
            ));
        }
        let frames = sh / sw;
        for i in 0..frames {
            cells.push(CellMeta {
                name: if i == 0 {
                    t.name.clone()
                } else {
                    format!("{}_{i}", t.name)
                },
                file: t.file.clone(),
                frame: i,
                anim_frames: if i == 0 { frames } else { 0 },
                frame_ticks,
                interpolate: t.interpolate,
                variation_count: 1,
                variation: None,
                world_tint: t.tint,
                icon_tint: t.icon_tint,
                fill_cutout_mips: t.fill_cutout_mips,
            });
        }
    }

    // The packed chunk vertex carries the tile id in a fixed-width field and
    // the shader's uv-rect table is sized to match, both derived from
    // [`MAX_TILES`], so the atlas cannot exceed it without a vertex-format
    // change. NOTE this counts CELLS, not manifest rows: an animated strip
    // expands to one cell per frame.
    let count = cells.len();
    if count > MAX_TILES {
        return Err(format!(
            "atlas has {count} tiles; the packed chunk vertex stores tile ids in {} bits (max {MAX_TILES} — see tile::MAX_TILES)",
            MAX_TILES.trailing_zeros(),
        ));
    }

    let mut names = Vec::with_capacity(count);
    let mut by_name: HashMap<&'static str, Tile> = HashMap::with_capacity(count);
    for (i, cell) in cells.iter().enumerate() {
        let name: &'static str = Box::leak(cell.name.clone().into_boxed_str());
        if by_name.insert(name, Tile(i as u16)).is_some() {
            return Err(format!("duplicate tile name '{name}'"));
        }
        names.push(name);
    }

    let need = |name: &str| -> Result<Tile, String> {
        by_name
            .get(name)
            .copied()
            .ok_or_else(|| format!("engine tile '{name}' missing from the atlas manifest"))
    };
    let mut destroy_stages = [Tile(0); 10];
    for (i, slot) in destroy_stages.iter_mut().enumerate() {
        *slot = need(&format!("destroy_stage_{i}"))?;
    }
    let engine = EngineTiles {
        grass_side: need("grass_side")?,
        grass_side_overlay: need("grass_side_overlay")?,
        dirt: need("dirt")?,
        chest_top: need("chest_top")?,
        chest_front: need("chest_front")?,
        chest_side: need("chest_side")?,
        chest_lid_front: need("chest_lid_front")?,
        chest_lid_side: need("chest_lid_side")?,
        chest_inside: need("chest_inside")?,
        chest_latch: need("chest_latch")?,
        destroy_stages,
        item_trail: need("item_trail")?,
    };

    Ok(TileData {
        cells,
        names,
        by_name,
        engine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_loads_and_engine_tiles_resolve() {
        // Forces the LazyLock (engine tiles included): a bad manifest set
        // panics right here.
        let d = data();
        assert!(!d.cells.is_empty() && d.cells.len() <= MAX_TILES);
        // Names round-trip.
        for tile in Tile::all() {
            assert_eq!(Tile::from_name(tile.name()), Some(tile));
        }
    }

    #[test]
    fn manifest_layers_merge_by_tile_name() {
        let (base, _) = crate::assets::read_base_text("textures/atlas.json")
            .expect("assets/textures/atlas.json must ship");
        // A pack layer retints an existing tile (replacing its row in place)
        // and appends a brand-new tile reusing an existing PNG.
        let layer = r#"{"tiles": [{"name": "stone", "file": "stone.png", "tint": "grass"}, {"name": "test_extra_tile", "file": "stone.png"}]}"#;
        let d = build(&[&base, layer]).expect("layered manifest builds");
        let stone = d.by_name["stone"];
        assert_eq!(d.cells[stone.index()].world_tint, Some(TileTint::Grass));
        let extra = d.by_name["test_extra_tile"];
        assert_eq!(
            extra.index(),
            d.cells.len() - 1,
            "new tiles append at the end"
        );
    }

    #[test]
    fn tint_columns_mirror_the_engine_rules() {
        // In-world (mesher) tint classes.
        for name in ["grass_top", "short_grass", "fern"] {
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), Some(TileTint::Grass), "{name}");
        }
        for name in ["water", "water_still", "water_flow"] {
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), Some(TileTint::Water), "{name}");
        }
        for name in ["oak_leaves", "spruce_leaves", "redwood_leaves"] {
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), Some(TileTint::Foliage), "{name}");
        }
        // Azalea keeps its baked colour in the WORLD but green in icons.
        {
            let name = "azalea_leaves";
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), None, "{name}");
            assert_eq!(t.icon_tint(), Some(TileTint::Foliage), "{name}");
        }
        // icon_tint defaults to world_tint.
        let oak = Tile::from_name("oak_leaves").unwrap();
        assert_eq!(oak.icon_tint(), Some(TileTint::Foliage));
        let stone = Tile::from_name("stone").unwrap();
        assert_eq!(stone.world_tint(), None);
        assert_eq!(stone.icon_tint(), None);
    }
}
