use std::collections::HashMap;

use serde::Deserialize;

use crate::assets::PackSet;

pub const MAX_TILES: usize = 2048;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tile(u16);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileTint {
    Grass,
    Foliage,
    Water,
    Fixed([u8; 3]),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariationSelect {
    Face,
    Cell,
}

pub const SPATIAL_HASH_AXIS_MULTIPLIERS: [u32; 3] = [0x8da6_b343, 0xd816_3841, 0xcb1a_b31f];
pub const SPATIAL_HASH_SALT_MULTIPLIER: u32 = 0x9e37_79b9;
pub const SPATIAL_HASH_MIX_MULTIPLIERS: [u32; 2] = [0x7feb_352d, 0x846c_a68b];

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
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    #[inline]
    pub fn id(self) -> u16 {
        self.0
    }

    #[inline]
    pub fn name(self) -> &'static str {
        data().names[self.index()]
    }

    pub fn from_name(name: &str) -> Option<Tile> {
        data().by_name.get(name).copied()
    }

    pub fn named(name: &str) -> Tile {
        Tile::from_name(name).unwrap_or_else(|| panic!("no tile named '{name}' in the atlas"))
    }

    #[inline]
    pub fn anim_frames(self) -> u32 {
        data().cells[self.index()].anim_frames
    }

    #[inline]
    pub fn variation_count(self) -> usize {
        data().cells[self.index()].variation_count as usize
    }

    #[inline]
    pub fn variation_select(self) -> Option<VariationSelect> {
        data().cells[self.index()].variation
    }

    #[inline]
    pub fn variation(self, seed: u32) -> Tile {
        Tile(self.0 + (seed % data().cells[self.index()].variation_count as u32) as u16)
    }

    /// The tile `offset` variations after this one (`variation` / `face_variation` resolved
    /// with an already-chosen index).
    #[inline]
    pub const fn variant(self, offset: u16) -> Tile {
        Tile(self.0 + offset)
    }

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

    #[inline]
    pub fn world_tint(self) -> Option<TileTint> {
        data().cells[self.index()].world_tint
    }

    #[inline]
    pub fn icon_tint(self) -> Option<TileTint> {
        let c = &data().cells[self.index()];
        c.icon_tint.or(c.world_tint)
    }

    #[inline]
    pub fn count() -> usize {
        data().cells.len()
    }

    pub fn all() -> impl Iterator<Item = Tile> {
        (0..Tile::count() as u16).map(Tile)
    }
}

pub fn install_map_colors(colors: Vec<[u8; 3]>) {
    let _ = crate::content::current().map_rgb.set(colors);
}

pub fn map_rgb(tile: Tile) -> [u8; 3] {
    crate::content::try_current()
        .and_then(|content| content.registry().map_rgb.get())
        .and_then(|v| v.get(tile.index()))
        .copied()
        .unwrap_or([32, 32, 32])
}

pub struct EngineTiles {
    pub grass_side: Tile,
    pub grass_side_overlay: Tile,
    pub dirt: Tile,
    pub destroy_stages: [Tile; 10],
    pub item_trail: Tile,
}

#[inline]
pub fn engine() -> &'static EngineTiles {
    &data().engine
}

pub const TICKS_PER_SECOND: f32 = 20.0;

pub struct CellMeta {
    pub name: String,
    pub file: String,
    pub frame: u32,
    pub anim_frames: u32,
    pub frame_ticks: f32,
    pub interpolate: bool,
    pub variation_count: u16,
    pub variation: Option<VariationSelect>,
    pub world_tint: Option<TileTint>,
    pub icon_tint: Option<TileTint>,
    pub fill_cutout_mips: bool,
}

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
    #[serde(default)]
    anim: bool,
    #[serde(default)]
    frame_ticks: Option<f32>,
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
    #[serde(default)]
    variants: Vec<String>,
    #[serde(default)]
    variation: Option<VariationSelect>,
}

impl RawTile {
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

pub(crate) struct TileData {
    cells: Vec<CellMeta>,
    names: Vec<&'static str>,
    by_name: HashMap<&'static str, Tile>,
    engine: EngineTiles,
}

pub(crate) fn load(packs: &PackSet) -> Result<TileData, String> {
    let layers = packs.read_asset_layers("textures/atlas.json");
    if layers.is_empty() {
        return Err(format!(
            "not found (searched {:?}); the game cannot run without its texture atlas",
            packs.candidate_paths("textures/atlas.json")
        ));
    }
    for (_, path) in &layers {
        log::info!("atlas manifest layer: {}", path.display());
    }
    let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
    build(&texts, packs)
}

#[cfg(test)]
mod variation_tests;

#[inline]
fn data() -> &'static TileData {
    crate::content::current().tiles()
}

fn image_dimensions(file: &str, packs: &PackSet) -> Result<(u32, u32), String> {
    let rel = format!("textures/{file}");
    let (bytes, _) = packs
        .read_bytes(&rel)
        .ok_or_else(|| format!("missing texture '{rel}' (searched the asset roots)"))?;
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("failed to read '{rel}': {e}"))?
        .into_dimensions()
        .map_err(|e| format!("failed to decode '{rel}' header: {e}"))
}

fn build(manifests: &[&str], packs: &PackSet) -> Result<TileData, String> {
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
        let (sw, sh) = image_dimensions(&t.file, packs)?;
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
        let d = data();
        assert!(!d.cells.is_empty() && d.cells.len() <= MAX_TILES);
        for tile in Tile::all() {
            assert_eq!(Tile::from_name(tile.name()), Some(tile));
        }
    }

    #[test]
    fn manifest_layers_merge_by_tile_name() {
        let (base, _) = crate::assets::read_base_text("textures/atlas.json")
            .expect("assets/textures/atlas.json must ship");
        let layer = r#"{"tiles": [{"name": "stone", "file": "stone.png", "tint": "grass"}, {"name": "test_extra_tile", "file": "stone.png"}]}"#;
        let d = build(&[&base, layer], crate::content::current().packs())
            .expect("layered manifest builds");
        let stone = d.by_name["stone"];
        assert_eq!(d.cells[stone.index()].world_tint, Some(TileTint::Grass));
        let extra = d.by_name["test_extra_tile"];
        assert_eq!(
            extra.index(),
            d.cells.len() - 1,
            "new tiles append at the end"
        );
    }
}
