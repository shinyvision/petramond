//! Fixed-biome foliage tinting for the **out-of-world** model3d renders (held
//! item, dropped item-entity cubes, hotbar/inventory icons).
//!
//! The chunk mesher ([`crate::mesh::builder`]) biome-tints grass tops / short
//! grass / ferns by the column's grass colour, leaves by its foliage colour, and
//! renders grass-block SIDES as an untinted dirt tile with a biome-tinted
//! grayscale `grass_block_side_overlay` composited on top (vertex overlay bits).
//! Icons / held items / dropped cubes have **no biome context**, so they pick a
//! single fixed temperate grass/foliage colour and classify each *tile* exactly
//! the way the mesher does (`mesh::builder::tile_tint` + the grass-side special
//! case) so a held grass block / leaf icon / dropped fern reads green like the
//! world block instead of gray.
//!
//! This is the single source of truth for that classification; both
//! [`super::block_model`] (CPU vertex packing) and `model3d.wgsl` (GPU composite)
//! rely on it staying in lock-step with the mesher.

use crate::atlas::Tile;
use crate::biome::Biome;

/// Fixed temperate grass colour for out-of-world tints. Plains is the canonical
/// default temperate biome (`biome::data::TEMPERATE_DRY_DEFAULT`), so its grass
/// colour is what an icon/held grass block greens to.
#[inline]
pub fn default_grass_color() -> [f32; 3] {
    Biome::Plains.grass_color()
}

/// Fixed temperate foliage colour for out-of-world tints (Plains foliage colour),
/// used to tint all `*Leaves` tiles.
#[inline]
pub fn default_foliage_color() -> [f32; 3] {
    Biome::Plains.foliage_color()
}

/// White (no tint) for everything that the mesher leaves untinted.
pub const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

/// The per-face material for an out-of-world render: which tile to sample, an
/// optional grayscale overlay tile composited on top (grass-block side), and the
/// RGB tint multiplied into the (overlay or base) colour.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FaceMaterial {
    /// Base tile sampled by the fragment shader.
    pub base_tile: Tile,
    /// Overlay tile composited over `base_tile` by its own alpha, tinted by
    /// `tint`. `None` for every non-grass-side face.
    pub overlay_tile: Option<Tile>,
    /// Tint multiplied into the base (or overlay) colour. `NO_TINT` for blocks the
    /// mesher does not tint.
    pub tint: [f32; 3],
}

/// Classify one face `tile` (as produced by `Block::tiles()`) into its
/// out-of-world [`FaceMaterial`], mirroring the chunk mesher:
/// - `GrassTop` / `ShortGrass` / `Fern` -> grass tint, no overlay.
/// - any `*Leaves` -> foliage tint, no overlay.
/// - `GrassSide` (the pre-baked grass-block side) -> dirt base + tinted
///   `GrassSideOverlay`, matching the mesher's grass-side compositing so the side
///   greens to match the top instead of showing the stale pre-greened texture.
/// - everything else (dirt, stone, logs, sand, flowers, mushrooms, cactus, dead
///   bush, ...) -> the tile untinted.
#[inline]
pub fn face_material(tile: Tile) -> FaceMaterial {
    match tile {
        Tile::GrassTop | Tile::ShortGrass | Tile::Fern => FaceMaterial {
            base_tile: tile,
            overlay_tile: None,
            tint: default_grass_color(),
        },
        Tile::GrassSide => FaceMaterial {
            base_tile: Tile::Dirt,
            overlay_tile: Some(Tile::GrassSideOverlay),
            tint: default_grass_color(),
        },
        Tile::OakLeaves
        | Tile::AcaciaLeaves
        | Tile::BirchLeaves
        | Tile::DarkOakLeaves
        | Tile::JungleLeaves
        | Tile::MangroveLeaves
        | Tile::SpruceLeaves
        | Tile::RedwoodLeaves
        | Tile::CherryLeaves
        | Tile::AzaleaLeaves => FaceMaterial {
            base_tile: tile,
            overlay_tile: None,
            tint: default_foliage_color(),
        },
        _ => FaceMaterial {
            base_tile: tile,
            overlay_tile: None,
            tint: NO_TINT,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_top_short_grass_and_fern_get_grass_tint_no_overlay() {
        for tile in [Tile::GrassTop, Tile::ShortGrass, Tile::Fern] {
            let m = face_material(tile);
            assert_eq!(m.base_tile, tile);
            assert_eq!(m.overlay_tile, None);
            assert_eq!(m.tint, default_grass_color());
            assert_ne!(m.tint, NO_TINT, "{tile:?} must be tinted green");
        }
    }

    #[test]
    fn grass_side_becomes_dirt_plus_tinted_overlay() {
        let m = face_material(Tile::GrassSide);
        assert_eq!(m.base_tile, Tile::Dirt);
        assert_eq!(m.overlay_tile, Some(Tile::GrassSideOverlay));
        assert_eq!(m.tint, default_grass_color());
    }

    #[test]
    fn all_leaves_get_foliage_tint() {
        for tile in [
            Tile::OakLeaves,
            Tile::AcaciaLeaves,
            Tile::BirchLeaves,
            Tile::DarkOakLeaves,
            Tile::JungleLeaves,
            Tile::MangroveLeaves,
            Tile::SpruceLeaves,
            Tile::CherryLeaves,
            Tile::AzaleaLeaves,
        ] {
            let m = face_material(tile);
            assert_eq!(m.base_tile, tile);
            assert_eq!(m.overlay_tile, None);
            assert_eq!(m.tint, default_foliage_color());
        }
    }

    #[test]
    fn non_foliage_tiles_stay_untinted() {
        for tile in [
            Tile::Dirt,
            Tile::Stone,
            Tile::Sand,
            Tile::OakLogSide,
            Tile::OakLogTop,
            Tile::Poppy,
            Tile::Dandelion,
            Tile::RedMushroom,
            Tile::DeadBush,
            Tile::CactusSide,
        ] {
            let m = face_material(tile);
            assert_eq!(m.base_tile, tile);
            assert_eq!(m.overlay_tile, None);
            assert_eq!(m.tint, NO_TINT, "{tile:?} must stay untinted");
        }
    }
}
