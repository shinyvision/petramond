use petramond_world::biome::Biome;
use petramond_world::tile::{Tile, TileTint};

#[inline]
pub fn default_grass_color() -> [f32; 3] {
    Biome::PLAINS.grass_color()
}

#[inline]
pub fn default_foliage_color() -> [f32; 3] {
    Biome::PLAINS.foliage_color()
}

pub const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FaceMaterial {
    pub base_tile: Tile,
    pub overlay_tile: Option<Tile>,
    pub tint: [f32; 3],
}

#[inline]
pub fn face_material(tile: Tile) -> FaceMaterial {
    let e = petramond_world::tile::engine();
    if tile == e.grass_side {
        return FaceMaterial {
            base_tile: e.dirt,
            overlay_tile: Some(e.grass_side_overlay),
            tint: default_grass_color(),
        };
    }
    match tile.icon_tint() {
        Some(TileTint::Grass) => FaceMaterial {
            base_tile: tile,
            overlay_tile: None,
            tint: default_grass_color(),
        },
        Some(TileTint::Foliage) => FaceMaterial {
            base_tile: tile,
            overlay_tile: None,
            tint: default_foliage_color(),
        },
        Some(TileTint::Fixed(rgb)) => FaceMaterial {
            base_tile: tile,
            overlay_tile: None,
            tint: rgb.map(|c| f32::from(c) / 255.0),
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

    fn t(name: &str) -> Tile {
        Tile::from_name(name).unwrap_or_else(|| panic!("missing tile '{name}'"))
    }

    #[test]
    fn grass_top_short_grass_and_fern_get_grass_tint_no_overlay() {
        for tile in [t("grass_top"), t("short_grass"), t("fern")] {
            let m = face_material(tile);
            assert_eq!(m.base_tile, tile);
            assert_eq!(m.overlay_tile, None);
            assert_eq!(m.tint, default_grass_color());
            assert_ne!(m.tint, NO_TINT, "{tile:?} must be tinted green");
        }
    }

    #[test]
    fn grass_side_becomes_dirt_plus_tinted_overlay() {
        let m = face_material(t("grass_side"));
        assert_eq!(m.base_tile, t("dirt"));
        assert_eq!(m.overlay_tile, Some(t("grass_side_overlay")));
        assert_eq!(m.tint, default_grass_color());
    }

    #[test]
    fn all_leaves_get_foliage_tint() {
        for tile in [
            t("oak_leaves"),
            t("acacia_leaves"),
            t("birch_leaves"),
            t("jungle_leaves"),
            t("spruce_leaves"),
            t("azalea_leaves"),
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
            t("dirt"),
            t("stone"),
            t("sand"),
            t("oak_log_side"),
            t("oak_log_top"),
            t("poppy"),
            t("dandelion"),
            t("red_mushroom"),
            t("dead_bush"),
            t("cactus_side"),
        ] {
            let m = face_material(tile);
            assert_eq!(m.base_tile, tile);
            assert_eq!(m.overlay_tile, None);
            assert_eq!(m.tint, NO_TINT, "{tile:?} must stay untinted");
        }
    }
}
