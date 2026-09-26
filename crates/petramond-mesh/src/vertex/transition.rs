//! A boundary face reuses the ordinary cube's tile/UV/overlay lanes.
//! Corner order, AO, skylight and RGB light remain untouched.
//!
//! Bit budget, without growing either the CPU or the GPU terrain vertex:
//!
//! - nine 4-bit material slots (the source, then its eight in-plane
//!   neighbours) = 36 bits, spread over the lanes a plain cube face no longer
//!   needs: the tile id (11), the has-overlay flag (1), `packed` bit 31 (1),
//!   the cell-local UV (10) and the overlay payload + dyed flag + `packed2`
//!   bit 31 (13);
//! - a 4-bit SET id: the low two bits ride the UV mode (modes 4..=7 are all
//!   "transition"), the high two bits ride the shade-index lane. A cube face's
//!   shade index is a pure function of its normal code, which the vertex
//!   still carries, so the shader recomputes it for transition vertices.
//!
//! That is [`MAX_SETS`] sets of [`MAX_MATERIALS_PER_SET`] materials per world.

use super::{
    retint, Vertex, CELL_UV_MASK, CELL_UV_U_SHIFT, DYED_FLAG2, OVERLAY_FLAG, OVERLAY_SHIFT2,
    SHADE_SHIFT, TILE_MASK, UV_MODE_SHIFT,
};
use petramond_world::texture_transition::{MAX_MATERIALS_PER_SET, MAX_SETS};

/// UV modes at or above this are transition faces; the low two bits of the
/// mode are the low two bits of the set id.
pub const UV_MODE_TRANSITION: u32 = 4;
const SET_LOW_BITS: u32 = 2;
const SET_LOW_MASK: u32 = (1 << SET_LOW_BITS) - 1;
const MATERIAL_BITS: u32 = 4;
const SLOTS: usize = 9;

/// The `packed` lanes a transition face overwrites.
const WORD1: u32 = TILE_MASK | (3 << SHADE_SHIFT) | (7 << UV_MODE_SHIFT) | OVERLAY_FLAG | (1 << 31);
/// The `packed2` lanes a transition face overwrites.
const WORD2: u32 = (0x3ff << CELL_UV_U_SHIFT) | DYED_FLAG2 | (0x7ff << OVERLAY_SHIFT2) | (1 << 31);

const _: () = assert!(MAX_SETS == 1 << (SET_LOW_BITS + 2));
const _: () = assert!(MAX_MATERIALS_PER_SET == (1 << MATERIAL_BITS) - 1);
const _: () = assert!(CELL_UV_MASK == 0x1f && 0x3ff == CELL_UV_MASK | (CELL_UV_MASK << 5));

/// One face's transition: which set renders it, and the local material of the
/// source followed by its eight neighbours in row-major order (centre
/// skipped); zero = no donor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub set: u8,
    pub grid: [u8; SLOTS],
}

impl Transition {
    /// Encode onto a freshly pushed cube face.
    pub fn apply(self, vertices: &mut [Vertex], tint: [f32; 3]) {
        debug_assert!((self.set as usize) < MAX_SETS);
        let mut data = 0u64;
        for (i, material) in self.grid.into_iter().enumerate() {
            debug_assert!((material as usize) <= MAX_MATERIALS_PER_SET);
            data |= (material as u64) << (i as u32 * MATERIAL_BITS);
        }
        let set = self.set as u32;
        let a = (data as u32 & TILE_MASK)
            | (((data >> 11) as u32 & 1) << OVERLAY_FLAG.trailing_zeros())
            | (((data >> 12) as u32 & 1) << 31)
            | ((UV_MODE_TRANSITION | (set & SET_LOW_MASK)) << UV_MODE_SHIFT)
            | ((set >> SET_LOW_BITS) << SHADE_SHIFT);
        let b = (((data >> 13) as u32 & 0x3ff) << CELL_UV_U_SHIFT)
            | (((data >> 23) as u32 & 0x1fff) << DYED_FLAG2.trailing_zeros());
        for v in vertices {
            v.packed = (v.packed & !WORD1) | a;
            v.packed2 = (v.packed2 & !WORD2) | b;
            v.tint = retint(v.tint, tint);
        }
    }

    /// Whether `v` carries a transition payload.
    #[inline]
    pub fn carried_by(v: &Vertex) -> bool {
        Self::carried_by_word(v.packed)
    }

    /// Whether a vertex whose `packed` word is `packed` carries a transition
    /// payload — for the sealed [`TerrainVertex`](super::TerrainVertex) streams
    /// as much as the builder's.
    #[inline]
    pub fn carried_by_word(packed: u32) -> bool {
        (packed >> UV_MODE_SHIFT) & 7 >= UV_MODE_TRANSITION
    }

    /// The Rust mirror of the shader's decode, for tests.
    #[cfg(test)]
    pub fn decode(v: &Vertex) -> Option<Self> {
        if !Self::carried_by(v) {
            return None;
        }
        let mode = (v.packed >> UV_MODE_SHIFT) & 7;
        let set = (mode & SET_LOW_MASK) | (((v.packed >> SHADE_SHIFT) & 3) << SET_LOW_BITS);
        let lo = (v.packed & TILE_MASK)
            | (((v.packed >> OVERLAY_FLAG.trailing_zeros()) & 1) << 11)
            | ((v.packed >> 31) << 12)
            | (((v.packed2 >> CELL_UV_U_SHIFT) & 0x3ff) << 13)
            | (((v.packed2 >> DYED_FLAG2.trailing_zeros()) & 0x1ff) << 23);
        let data = lo as u64 | ((v.packed2 >> 28) as u64) << 32;
        Some(Self {
            set: set as u8,
            grid: std::array::from_fn(|i| ((data >> (i as u32 * MATERIAL_BITS)) & 15) as u8),
        })
    }
}

/// The shader decode of [`Transition::apply`]'s payload, for the generated
/// `petramond::vertex` module: the 36 material bits reassembled as a low word
/// (tile lane, overlay flag, `packed` bit 31, cell-local UV, then the low bits
/// of the `packed2` tail) and a high nibble, plus the set id from the UV-mode
/// and shade lanes. Spelled from the same constants `apply` writes with.
pub(super) fn wgsl() -> String {
    let tile_bits = TILE_MASK.count_ones();
    let overlay = OVERLAY_FLAG.trailing_zeros();
    let bit31_at = tile_bits + 1;
    let cell_uv = 0x3ff_u32;
    let cell_uv_at = tile_bits + 2;
    let tail_at = cell_uv_at + cell_uv.count_ones();
    let tail_mask = (1u32 << (u32::BITS - tail_at)) - 1;
    let dyed = DYED_FLAG2.trailing_zeros();
    let hi_shift = dyed + tail_mask.count_ones();
    format!(
        "struct TransitionWords {{\n    lo: u32,\n    hi: u32,\n    set_id: u32,\n}}\n\
         fn transition_words(packed: u32, packed2: u32) -> TransitionWords {{\n    \
         let lo = (packed & {TILE_MASK:#X}u) | (((packed >> {overlay}u) & 0x1u) << {tile_bits}u)\n        \
         | ((packed >> 31u) << {bit31_at}u)\n        \
         | (((packed2 >> {CELL_UV_U_SHIFT}u) & {cell_uv:#X}u) << {cell_uv_at}u)\n        \
         | (((packed2 >> {dyed}u) & {tail_mask:#X}u) << {tail_at}u);\n    \
         let set_id = (((packed >> {UV_MODE_SHIFT}u) & 0x7u) - UV_MODE_TRANSITION)\n        \
         | (((packed >> {SHADE_SHIFT}u) & 0x3u) << {SET_LOW_BITS}u);\n    \
         return TransitionWords(lo, packed2 >> {hi_shift}u, set_id);\n}}\n"
    )
}

#[cfg(test)]
mod tests;
