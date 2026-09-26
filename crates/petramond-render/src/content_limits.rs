//! Content-driven GPU needs, checked against the adapter's limits before the
//! device is created — so a pack too large for the GPU fails naming the
//! content that overflowed, not with a texture-creation or device error deep
//! in renderer construction.

use petramond_world::item::ItemType;
use petramond_world::tile::Tile;

/// Bytes per uv-rect table row (see [`crate::uniforms::UV_RECT_BYTES`]).
const UV_RECT_BYTES: u64 = crate::uniforms::UV_RECT_BYTES;
/// Side of one icon-atlas cell, px.
const ICON_CELL: u32 = 64;

/// What the loaded content asks of the GPU.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContentNeeds {
    /// Atlas tiles (animation frames included).
    pub tiles: u32,
    /// Item types (each bakes an icon and its dyed twin).
    pub items: u32,
}

impl ContentNeeds {
    /// The needs of the content loaded in this process.
    pub fn loaded() -> Self {
        Self {
            tiles: Tile::count() as u32,
            items: ItemType::all().len() as u32,
        }
    }
}

/// One way the content overflows a limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shortfall {
    /// Whether the renderer cannot run at all (vs. degrading).
    pub fatal: bool,
    pub message: String,
}

/// Every limit `needs` overflows on a device with `limits`.
pub(crate) fn shortfalls(needs: ContentNeeds, limits: &wgpu::Limits) -> Vec<Shortfall> {
    let mut out = Vec::new();
    // The terrain tile array holds every tile and its dye-base twin.
    let layers = 2 * needs.tiles;
    if layers > limits.max_texture_array_layers {
        out.push(Shortfall {
            fatal: true,
            message: format!(
                "{} atlas tiles need {layers} texture-array layers (each tile plus its \
                 dye-base twin), but this GPU allows {} — at most {} tiles fit",
                needs.tiles,
                limits.max_texture_array_layers,
                limits.max_texture_array_layers / 2
            ),
        });
    }
    let uv_bytes = u64::from(needs.tiles) * UV_RECT_BYTES;
    if uv_bytes > u64::from(limits.max_storage_buffer_binding_size) {
        out.push(Shortfall {
            fatal: true,
            message: format!(
                "{} atlas tiles need a {uv_bytes}-byte uv table, but this GPU binds at \
                 most {} bytes",
                needs.tiles, limits.max_storage_buffer_binding_size
            ),
        });
    }
    let side = u64::from(limits.max_texture_dimension_2d / ICON_CELL);
    let cells = 2 * u64::from(needs.items);
    if cells > side * side {
        out.push(Shortfall {
            fatal: false,
            message: format!(
                "{} items need {cells} icon cells (each plus its dyed twin), but this GPU's \
                 {} px textures hold {}; the surplus icons render blank",
                needs.items,
                limits.max_texture_dimension_2d,
                side * side
            ),
        });
    }
    out
}

/// Check the loaded content against `limits` (the ones the device is created
/// with): degradations are logged, and a fatal overflow is returned with
/// every problem named, so bring-up fails with a typed error instead of a
/// panic.
pub(crate) fn check(limits: &wgpu::Limits) -> Result<(), String> {
    let found = shortfalls(ContentNeeds::loaded(), limits);
    for s in found.iter().filter(|s| !s.fatal) {
        log::error!("content exceeds a GPU limit: {}", s.message);
    }
    let fatal: Vec<_> = found
        .iter()
        .filter(|s| s.fatal)
        .map(|s| s.message.as_str())
        .collect();
    if fatal.is_empty() {
        Ok(())
    } else {
        Err(fatal.join("\n  "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(layers: u32, storage: u32, dim: u32) -> wgpu::Limits {
        wgpu::Limits {
            max_texture_array_layers: layers,
            max_storage_buffer_binding_size: storage,
            max_texture_dimension_2d: dim,
            ..wgpu::Limits::default()
        }
    }

    #[test]
    fn content_within_every_limit_has_no_shortfall() {
        let needs = ContentNeeds {
            tiles: 900,
            items: 800,
        };
        assert!(shortfalls(needs, &limits(2048, 1 << 27, 8192)).is_empty());
    }

    /// The tile array's twin layers are named with the tile count that fits.
    #[test]
    fn too_many_tiles_for_the_layer_limit_is_fatal_and_says_how_many_fit() {
        let needs = ContentNeeds {
            tiles: 1500,
            items: 10,
        };
        let found = shortfalls(needs, &limits(2048, 1 << 27, 8192));
        assert_eq!(found.len(), 1);
        assert!(found[0].fatal);
        assert!(found[0].message.contains("1500 atlas tiles"), "{}", found[0].message);
        assert!(found[0].message.contains("at most 1024 tiles"), "{}", found[0].message);
    }

    /// Icons degrade rather than stop the game.
    #[test]
    fn too_many_icons_degrades() {
        let needs = ContentNeeds {
            tiles: 10,
            items: 1000,
        };
        let found = shortfalls(needs, &limits(2048, 1 << 27, 2048));
        assert_eq!(found.len(), 1);
        assert!(!found[0].fatal);
        assert!(found[0].message.contains("1000 items"), "{}", found[0].message);
    }

    #[test]
    fn an_oversized_uv_table_is_fatal() {
        let needs = ContentNeeds {
            tiles: 100,
            items: 1,
        };
        let found = shortfalls(needs, &limits(2048, 1024, 8192));
        assert!(found.iter().any(|s| s.fatal && s.message.contains("uv table")));
    }
}
