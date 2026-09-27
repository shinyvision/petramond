use petramond_world::item::ItemType;
use petramond_world::tile::Tile;

const UV_RECT_BYTES: u64 = crate::uniforms::UV_RECT_BYTES;
const ICON_CELL: u32 = 64;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContentNeeds {
    pub tiles: u32,
    pub items: u32,
}

impl ContentNeeds {
    pub fn loaded() -> Self {
        Self {
            tiles: Tile::count() as u32,
            items: ItemType::all().len() as u32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shortfall {
    pub fatal: bool,
    pub message: String,
}

pub(crate) fn shortfalls(needs: ContentNeeds, limits: &wgpu::Limits) -> Vec<Shortfall> {
    let mut out = Vec::new();
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

    #[test]
    fn too_many_tiles_for_the_layer_limit_is_fatal_and_says_how_many_fit() {
        let needs = ContentNeeds {
            tiles: 1500,
            items: 10,
        };
        let found = shortfalls(needs, &limits(2048, 1 << 27, 8192));
        assert_eq!(found.len(), 1);
        assert!(found[0].fatal);
        assert!(
            found[0].message.contains("1500 atlas tiles"),
            "{}",
            found[0].message
        );
        assert!(
            found[0].message.contains("at most 1024 tiles"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn too_many_icons_degrades() {
        let needs = ContentNeeds {
            tiles: 10,
            items: 1000,
        };
        let found = shortfalls(needs, &limits(2048, 1 << 27, 2048));
        assert_eq!(found.len(), 1);
        assert!(!found[0].fatal);
        assert!(
            found[0].message.contains("1000 items"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn an_oversized_uv_table_is_fatal() {
        let needs = ContentNeeds {
            tiles: 100,
            items: 1,
        };
        let found = shortfalls(needs, &limits(2048, 1024, 8192));
        assert!(found
            .iter()
            .any(|s| s.fatal && s.message.contains("uv table")));
    }
}
