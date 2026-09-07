//! Static tile variation on the GPU: the spatial hash shared with the
//! mesher's per-face selection, and the per-cell family table, both emitted
//! from the registry at pipeline construction.

use petramond_world::tile::{
    self, VariationSelect, SPATIAL_HASH_AXIS_MULTIPLIERS, SPATIAL_HASH_MIX_MULTIPLIERS,
    SPATIAL_HASH_SALT_MULTIPLIER,
};
use std::fmt::Write;

pub(super) fn declarations() -> String {
    hash_declaration() + &family_declaration()
}

/// `spatial_hash(cell, salt)`, the WGSL twin of `tile::spatial_hash`.
pub(super) fn hash_declaration() -> String {
    let [mx, my, mz] = SPATIAL_HASH_AXIS_MULTIPLIERS;
    let [a, b] = SPATIAL_HASH_MIX_MULTIPLIERS;
    format!(
        "fn spatial_hash(cell: vec3<i32>, salt: u32) -> u32 {{\n\
         \x20   let p = bitcast<vec3<u32>>(cell);\n\
         \x20   var h = (p.x * {mx:#x}u) ^ (p.y * {my:#x}u) ^ (p.z * {mz:#x}u) ^ (salt * {SPATIAL_HASH_SALT_MULTIPLIER:#x}u);\n\
         \x20   h ^= h >> 16u;\n\
         \x20   h *= {a:#x}u;\n\
         \x20   h ^= h >> 15u;\n\
         \x20   h *= {b:#x}u;\n\
         \x20   return h ^ (h >> 16u);\n\
         }}\n"
    )
}

/// `block_variation_count(tile)`: how many alternatives a CELL-selecting
/// base tile heads; 1 for everything else (face selection happened on the CPU).
fn family_declaration() -> String {
    let mut text =
        String::from("fn block_variation_count(tile: u32) -> u32 {\n    switch tile {\n");
    for (id, cell) in tile::cells().iter().enumerate() {
        if cell.variation == Some(VariationSelect::Cell) {
            writeln!(
                text,
                "        case {id}u: {{ return {}u; }}",
                cell.variation_count
            )
            .unwrap();
        }
    }
    text.push_str("        default: { return 1u; }\n    }\n}\n");
    text
}

#[cfg(test)]
mod tests;
