mod chunk_mesh;
pub mod transition;
pub mod wgsl;
pub use chunk_mesh::{ChunkMesh, QuadLayer};
use petramond_world::light::BlockLight6;

pub use petramond_world::shade::SHADES;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub tint: u32,
    /// Folded tile + corner + shade + overlay + AO + SKY light. [`pack_vertex`] is
    /// the sole owner of this bit layout (see its doc); the vertex shader decodes
    /// it (selecting uv from the CPU-uploaded `tile_uv()` table — never recomputing
    /// — and light from `SHADES * AO`).
    pub packed: u32,
    pub packed2: u32,
}

pub const TERRAIN_POS_SCALE: f32 = 64.0;

/// Packed-column terrain vertex: **20 bytes**. `pos` is the mesh-space position
/// (column-local XZ + world Y) in [`TERRAIN_POS_SCALE`] fixed point (`i16`);
/// the draw binds the column's integer world origin as an instance-step
/// attribute, and the terrain VS offsets by that origin minus the render
/// origin in integers, so the large part never reaches a float. The mesh
/// builder emits [`Vertex`]; the mesh worker converts when it seals the mesh
/// ([`ChunkMesh::seal`]), so the renderer only copies these.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainVertex {
    pub pos: [i16; 3],
    pub _pad: i16,
    pub tint: u32,
    pub packed: u32,
    pub packed2: u32,
}

#[inline]
pub(crate) fn round_i32(x: f32) -> i32 {
    const LIMIT: f32 = (1 << 30) as f32;
    let x = x.clamp(-LIMIT, LIMIT);
    let t = x as i32;
    let frac = x - t as f32;
    if frac >= 0.5 {
        t + 1
    } else if frac <= -0.5 {
        t - 1
    } else {
        t
    }
}

impl TerrainVertex {
    #[inline]
    pub fn from_mesh(v: &Vertex) -> Self {
        // Round half away from zero: `x + copysign(0.5, x)` truncated is exact for every
        // magnitude the i16 lane can hold (the sum is representable below 2^22), and `as i32`
        // saturates beyond it and maps NaN to 0, matching `round_i32` + clamp without its
        // branches or the array-map iterator.
        #[inline(always)]
        fn quant(p: f32) -> i16 {
            let x = p * TERRAIN_POS_SCALE;
            let r = (x + 0.5f32.copysign(x)) as i32;
            r.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        }
        Self {
            pos: [quant(v.pos[0]), quant(v.pos[1]), quant(v.pos[2])],
            _pad: 0,
            tint: v.tint,
            packed: v.packed,
            packed2: v.packed2,
        }
    }

    #[cfg(test)]
    pub fn to_mesh(self) -> [f32; 3] {
        self.pos.map(|p| p as f32 / TERRAIN_POS_SCALE)
    }
}

#[cfg(test)]
mod terrain_vertex_tests {
    use super::*;

    #[test]
    fn round_i32_matches_f32_round() {
        let mut x = -70_000.0f32;
        while x < 70_000.0 {
            for v in [
                x,
                x + 0.5,
                x - 0.5,
                x + 0.49999997,
                x.next_up(),
                x.next_down(),
            ] {
                assert_eq!(round_i32(v), v.round() as i32, "{v}");
            }
            x += 0.37;
        }
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5, -0.0] {
            assert_eq!(
                round_i32(v),
                (v.round() as i32).clamp(-(1 << 30), 1 << 30),
                "{v}"
            );
        }
    }

    #[test]
    fn terrain_quantization_matches_round_i32() {
        let mut x = -600.0f32;
        while x < 600.0 {
            for p in [
                x,
                x + 1.0 / 128.0,
                x - 1.0 / 128.0,
                x.next_up(),
                x.next_down(),
            ] {
                let want =
                    round_i32(p * TERRAIN_POS_SCALE).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                let got = TerrainVertex::from_mesh(&Vertex {
                    pos: [p; 3],
                    tint: 0,
                    packed: 0,
                    packed2: 0,
                })
                .pos[0];
                assert_eq!(got, want, "{p}");
            }
            x += 0.0173;
        }
        for p in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e9, -1e9] {
            let want =
                round_i32(p * TERRAIN_POS_SCALE).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let got = TerrainVertex::from_mesh(&Vertex {
                pos: [p; 3],
                tint: 0,
                packed: 0,
                packed2: 0,
            })
            .pos[0];
            assert_eq!(got, want, "{p}");
        }
    }

    #[test]
    fn terrain_pos_quantizes_within_half_unit() {
        let v = Vertex {
            pos: [3.125, 64.5, 0.0625],
            tint: 0xFF00_00FF,
            packed: 1,
            packed2: 2,
        };
        let t = TerrainVertex::from_mesh(&v);
        let back = t.to_mesh();
        for i in 0..3 {
            assert!(
                (back[i] - v.pos[i]).abs() <= 0.5 / TERRAIN_POS_SCALE + f32::EPSILON,
                "axis {i}: {back:?} vs {:?}",
                v.pos
            );
        }
        assert_eq!(t.tint, v.tint);
        assert_eq!(t.packed, v.packed);
        assert_eq!(t.packed2, v.packed2);
        assert_eq!(std::mem::size_of::<TerrainVertex>(), 20);
    }

    /// The three GPU words are the `petramond::vertex` shader ABI: engine
    /// shaders decode them through the module [`wgsl::layout`] generates, and
    /// pack shaders may import it too. This spells that ABI out — BOTH packed
    /// words AND the `Unorm8x4` tint word (alpha lane included) — and
    /// round-trips every field at its extremes, including a tile id ABOVE the
    /// old 8-bit ceiling and the overlay payload in its `packed2` home.
    ///
    /// The literals below are deliberately spelled out rather than derived
    /// from the constants: moving a lane changes what every pack shader built
    /// against the module reads, so it must be a deliberate edit here too.
    #[test]
    fn packed_words_round_trip_through_the_shaders_decode() {
        let cases = [
            (0u32, 0u32, 0u32, 0u32, 0u32, false, 0u32),
            (255, 3, 3, 3, 63, true, 0x7FF),
            (256, 1, 2, 1, 31, false, 1),
            (TILE_MASK, 2, 1, 2, 17, true, OVERLAY_MASK),
        ];
        for (tile, corner, shade, ao, sky, has_overlay, overlay) in cases {
            let packed = pack_vertex(tile, corner, shade, has_overlay, ao, sky);
            let packed2 =
                BlockLight6::grey(45).packed2_bits() | pack_overlay(overlay) | pack_normal_code(5);
            assert_eq!(packed & 0x7FF, tile, "tile");
            assert_eq!((packed >> 11) & 0x3, corner, "corner");
            assert_eq!((packed >> 13) & 0x3, shade, "shade");
            assert_eq!((packed >> 15) & 0x3, ao, "ao");
            assert_eq!((packed >> 17) & 0x3F, sky, "sky");
            assert_eq!((packed >> 26) & 0x1, has_overlay as u32, "has_overlay");
            assert_eq!((packed2 >> 20) & 0x7FF, overlay, "overlay payload");
            assert_eq!(packed2 & 0x3F, 45, "block light");
            assert_eq!((packed2 >> 16) & 0x7, 5, "normal code");
            let with_mode = packed | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT);
            assert_eq!((with_mode >> 23) & 0x7, UV_MODE_CELL_LOCAL, "uv mode");
            assert_eq!(with_mode & !(0x7 << 23), packed, "uv mode overlaps a field");
        }

        for (u16ths, v16ths) in [(0u32, 0u32), (16, 0), (0, 16), (16, 16), (7, 11)] {
            let packed2 =
                BlockLight6::grey(63).packed2_bits() | pack_cell_uv(u16ths, v16ths) | DYED_FLAG2;
            assert_eq!((packed2 >> 6) & 0x1F, u16ths, "cell-local u");
            assert_eq!((packed2 >> 11) & 0x1F, v16ths, "cell-local v");
            assert_eq!((packed2 >> 19) & 0x1, 1, "dyed flag");
            assert_eq!(packed2 & 0x3F, 63, "block light survives the uv lanes");
            assert_eq!((packed2 >> 20) & 0x7FF, 0, "uv must not reach the overlay");
        }

        for turn in 0..4u32 {
            let light = BlockLight6::new(63, 12, 40);
            let packed = pack_vertex(TILE_MASK, 3, 3, true, 3, 63)
                | light.packed_bits()
                | pack_uv_turn(turn);
            let packed2 = BlockLight6::grey(63).packed2_bits()
                | pack_overlay(OVERLAY_MASK)
                | pack_cell_uv(16, 16)
                | pack_uv_turn2(turn);
            assert_eq!(
                ((packed >> 31) & 0x1) | (((packed2 >> 31) & 0x1) << 1),
                turn,
                "uv turn {turn}"
            );
            assert_eq!(packed & 0x7FF, TILE_MASK, "tile survives the turn bit");
            assert_eq!(
                (packed >> 27) & 0xF,
                (light.packed_bits() >> 27) & 0xF,
                "chroma nibble survives the turn bit"
            );
            assert_eq!(
                decode_vertex_light(&Vertex {
                    pos: [0.0; 3],
                    tint: light.tint_word([1.0; 3]),
                    packed,
                    packed2
                }),
                light,
                "light decode survives the turn bits"
            );
            assert_eq!(
                (packed2 >> 20) & 0x7FF,
                OVERLAY_MASK,
                "overlay survives the turn bit"
            );
            assert_eq!(
                (packed2 >> 6) & 0x1F,
                16,
                "cell-local uv survives the turn bit"
            );
            assert_eq!(packed2 & 0x3F, 63, "block light survives the turn bit");
        }

        for rgb in [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.82, 0.52],
            [0.25, 0.5, 0.75],
        ] {
            let tint = pack_tint(rgb);
            let lane = |i: u32| ((tint >> (8 * i)) & 0xFF) as f32 / 255.0;
            for c in 0..3u32 {
                assert!(
                    (lane(c) - rgb[c as usize]).abs() <= 0.5 / 255.0 + 1e-6,
                    "tint lane {c}: {} vs {rgb:?}",
                    lane(c)
                );
            }
            assert_eq!(
                unpack_tint(tint),
                [lane(0), lane(1), lane(2)],
                "unpack_tint"
            );
            assert_eq!(tint >> 24, 0, "colourless light writes no chroma");
            assert_eq!(
                tint & !(0xFF << TINT_ALPHA_SHIFT),
                tint & 0x00FF_FFFF,
                "the alpha lane sits above the three colour lanes"
            );
            let lit = BlockLight6::new(63, 12, 40).tint_word(rgb);
            assert_eq!(retint(lit, [0.5, 0.5, 0.5]) >> 24, lit >> 24);
            assert_eq!(retint(lit, rgb), lit);
        }

        assert_eq!(std::mem::offset_of!(Vertex, tint), 12, "Vertex tint offset");
        assert_eq!(
            std::mem::offset_of!(TerrainVertex, tint),
            8,
            "TerrainVertex tint offset"
        );
        assert_eq!(std::mem::size_of::<Vertex>(), 24);
        assert_eq!(std::mem::size_of::<TerrainVertex>(), 20);
    }

    /// The block light's three channels are split across THREE words, so an
    /// emitter's colour survives only if every destination is written. This
    /// round-trips the split through the same decode the shaders perform, and
    /// pins the two properties the split rests on: colourless light writes NO
    /// chroma bits (so a path that drops the chroma degrades to grey rather
    /// than red), and black has exactly one spelling.
    #[test]
    fn block_light_colour_survives_the_three_way_vertex_split() {
        let cases = [
            BlockLight6::DARK,
            BlockLight6::grey(63),
            BlockLight6::grey(1),
            BlockLight6::new(63, 0, 0),
            BlockLight6::new(0, 0, 63),
            BlockLight6::new(27, 17, 63),
            BlockLight6::new(63, 52, 33),
        ];
        for light in cases {
            for rgb in [[1.0, 1.0, 1.0], [0.4, 0.9, 0.2]] {
                let v = Vertex {
                    pos: [0.0; 3],
                    tint: light.tint_word(rgb),
                    packed: pack_vertex(TILE_MASK, 3, 3, true, 3, 63)
                        | light.packed_bits()
                        | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
                    packed2: light.packed2_bits()
                        | pack_cell_uv(16, 16)
                        | pack_normal_code(6)
                        | DYED_FLAG2
                        | pack_overlay(OVERLAY_MASK),
                };
                assert_eq!(decode_vertex_light(&v), light, "tint {rgb:?}");
                assert_eq!(unpack_tint(v.tint), unpack_tint(pack_tint(rgb)));
                assert_eq!(v.packed & TILE_MASK, TILE_MASK);
                assert_eq!((v.packed >> 26) & 0x1, 1, "has-overlay");
                assert_eq!((v.packed2 >> 20) & OVERLAY_MASK, OVERLAY_MASK);
            }
        }
        for v in 0..=63u32 {
            let grey = BlockLight6::grey(v);
            assert_eq!(grey.packed_bits(), 0, "grey level {v} spent packed bits");
            assert_eq!(grey.tint_word([1.0; 3]), pack_tint([1.0; 3]));
        }
        assert_eq!(BlockLight6::DARK.packed2_bits(), 0);
    }

    #[test]
    fn the_light_decoding_shaders_read_the_tint_alpha_lane() {
        for (name, src) in [
            (
                "block.wgsl",
                include_str!("../../petramond-render/shaders/block.wgsl"),
            ),
            (
                "model3d.wgsl",
                include_str!("../../petramond-render/shaders/model3d.wgsl"),
            ),
        ] {
            assert!(
                src.contains("tint: vec4<f32>"),
                "{name} must declare the tint attribute vec4 to reach the chroma lane"
            );
            assert!(
                src.contains("tint.a"),
                "{name} no longer reads the chroma lane out of the tint alpha"
            );
        }
    }

    #[test]
    fn the_greedy_span_round_trips_through_the_shaders_decode() {
        for (w, h) in [(1u32, 1u32), (16, 1), (1, 16), (16, 16), (5, 11)] {
            let packed2 =
                BlockLight6::grey(9).packed2_bits() | pack_overlay(pack_greedy_span(w, h));
            assert_eq!(unpack_greedy_span(packed2), (w, h));
            assert_eq!(((packed2 >> 20) & 0xF) + 1, w, "gw");
            assert_eq!(((packed2 >> 24) & 0xF) + 1, h, "gh");
            assert_eq!(
                (packed2 >> OVERLAY_SHIFT2) & OVERLAY_MASK == 0,
                (w, h) == (1, 1),
            );
        }
    }

    #[test]
    fn the_tile_field_spans_the_shared_tile_id_space() {
        assert_eq!(MAX_TILES, TILE_MASK as usize + 1);
    }
}

pub const TINT_ALPHA_SHIFT: u32 = 24;

#[inline]
pub fn pack_tint(rgb: [f32; 3]) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    q(rgb[0]) | (q(rgb[1]) << 8) | (q(rgb[2]) << 16)
}

#[inline]
pub fn retint(tint_word: u32, rgb: [f32; 3]) -> u32 {
    pack_tint(rgb) | (tint_word & (0xFF << TINT_ALPHA_SHIFT))
}

/// The block light's CHROMA word — its GREEN and BLUE channels — split across
/// the two homes with spare bits: the low 8 in the `Vertex::tint` alpha lane,
/// the high 4 in `Vertex::packed` bits 27..31.
///
/// A third vertex word is not affordable (+20% terrain VRAM, upload and draw),
/// and no single lane has 12 contiguous free bits, so the split is the price of
/// colour. It is made safe by storing each secondary channel XOR the RED
/// channel: COLOURLESS light is then exactly zero, which means the canonical
/// white vertex writes no chroma bits at all, black has one spelling, and a path that carries
/// only part of the split degrades to colourless light rather than a red cast.
pub const CHROMA_HI_SHIFT: u32 = 27;
pub const CHROMA_HI_MASK: u32 = 0xF;
const CHROMA_LO_BITS: u32 = 8;

pub trait BlockLightVertexExt: Copy {
    fn chroma(self) -> u32;
    fn packed_bits(self) -> u32;
    fn packed2_bits(self) -> u32;
    fn tint_word(self, rgb: [f32; 3]) -> u32;
}

impl BlockLightVertexExt for BlockLight6 {
    #[inline]
    fn chroma(self) -> u32 {
        (self.g() ^ self.r()) | ((self.b() ^ self.r()) << 6)
    }

    #[inline]
    fn packed_bits(self) -> u32 {
        ((self.chroma() >> CHROMA_LO_BITS) & CHROMA_HI_MASK) << CHROMA_HI_SHIFT
    }

    #[inline]
    fn packed2_bits(self) -> u32 {
        self.r() & BLOCK_LIGHT_MASK
    }

    #[inline]
    fn tint_word(self, rgb: [f32; 3]) -> u32 {
        pack_tint(rgb) | ((self.chroma() & 0xFF) << TINT_ALPHA_SHIFT)
    }
}

#[inline]
pub fn decode_vertex_light(v: &Vertex) -> BlockLight6 {
    let chroma = ((v.tint >> TINT_ALPHA_SHIFT) & 0xFF)
        | (((v.packed >> CHROMA_HI_SHIFT) & CHROMA_HI_MASK) << CHROMA_LO_BITS);
    let r = v.packed2 & BLOCK_LIGHT_MASK;
    BlockLight6::new(r, (chroma & 0x3F) ^ r, ((chroma >> 6) & 0x3F) ^ r)
}

#[inline]
pub fn unpack_tint(tint: u32) -> [f32; 3] {
    [
        (tint & 0xFF) as f32 / 255.0,
        ((tint >> 8) & 0xFF) as f32 / 255.0,
        ((tint >> 16) & 0xFF) as f32 / 255.0,
    ]
}

#[inline]
pub fn pack_vertex(
    tile: u32,
    corner: u32,
    shade_idx: u32,
    has_overlay: bool,
    ao: u32,
    light: u32,
) -> u32 {
    debug_assert!(tile <= TILE_MASK, "tile id exceeds the packed field");
    (tile & TILE_MASK)
        | (corner << CORNER_SHIFT)
        | (shade_idx << SHADE_SHIFT)
        | (ao << AO_SHIFT)
        | (light << SKY_SHIFT)
        | if has_overlay { OVERLAY_FLAG } else { 0 }
}

pub const TILE_BITS: u32 = petramond_world::tile::MAX_TILES.trailing_zeros();
pub const TILE_MASK: u32 = (1 << TILE_BITS) - 1;
pub use petramond_world::tile::MAX_TILES;
pub const CORNER_SHIFT: u32 = 11;
pub const SHADE_SHIFT: u32 = 13;
pub const AO_SHIFT: u32 = 15;
pub const SKY_SHIFT: u32 = 17;

pub const OVERLAY_FLAG: u32 = 1 << 26;

pub const OVERLAY_SHIFT2: u32 = 20;
pub const OVERLAY_MASK: u32 = (1 << TILE_BITS) - 1;

#[inline]
pub fn pack_overlay(payload: u32) -> u32 {
    debug_assert!(payload <= OVERLAY_MASK, "overlay payload exceeds its field");
    (payload & OVERLAY_MASK) << OVERLAY_SHIFT2
}

#[inline]
pub fn pack_greedy_span(w: u32, h: u32) -> u32 {
    debug_assert!(
        (1..=16).contains(&w) && (1..=16).contains(&h),
        "span 1..=16"
    );
    ((w - 1) & 0xF) | (((h - 1) & 0xF) << 4)
}

#[cfg(test)]
#[inline]
pub fn unpack_greedy_span(packed2: u32) -> (u32, u32) {
    let payload = (packed2 >> OVERLAY_SHIFT2) & OVERLAY_MASK;
    ((payload & 0xF) + 1, ((payload >> 4) & 0xF) + 1)
}

pub const BLOCK_LIGHT_MASK: u32 = 0x3F;

pub const DYED_FLAG2: u32 = 1 << 19;

#[inline]
pub fn pack_normal_code(code: u32) -> u32 {
    (code & NORMAL_CODE_MASK) << NORMAL_CODE_SHIFT
}

pub const NORMAL_CODE_SHIFT: u32 = 16;
pub const NORMAL_CODE_MASK: u32 = 0x7;

#[inline]
pub fn pack_cell_uv(u16ths: u32, v16ths: u32) -> u32 {
    debug_assert!(u16ths <= 16 && v16ths <= 16);
    ((u16ths & CELL_UV_MASK) << CELL_UV_U_SHIFT) | ((v16ths & CELL_UV_MASK) << CELL_UV_V_SHIFT)
}

pub const CELL_UV_U_SHIFT: u32 = 6;
pub const CELL_UV_V_SHIFT: u32 = 11;
pub const CELL_UV_MASK: u32 = 0x1F;

#[inline]
pub fn pack_fluid_face(medium: u32, flow: bool) -> u32 {
    debug_assert!(medium < MAX_FLUID_MEDIA, "fluid medium exceeds its lane");
    (((medium + 1) & FLUID_MEDIUM_MASK) << FLUID_MEDIUM_SHIFT)
        | if flow { FLUID_FLOW_FLAG2 } else { 0 }
}

pub const FLUID_MEDIUM_SHIFT: u32 = CELL_UV_U_SHIFT;
pub const FLUID_MEDIUM_MASK: u32 = 0x1FF;
pub const FLUID_FLOW_FLAG2: u32 = 1 << 15;
pub const MAX_FLUID_MEDIA: u32 = FLUID_MEDIUM_MASK;

/// Packed UV mode field, shared by `block.wgsl` and dynamic block geometry.
pub const UV_MODE_SHIFT: u32 = 23;
pub const UV_MODE_NONE: u32 = 0;
pub const UV_MODE_THIN_U: u32 = 1;
pub const UV_MODE_THIN_V: u32 = 2;
pub const UV_MODE_CELL_LOCAL: u32 = 3;

/// A plain cube face's row-declared UV quarter turn (0..4) — the shader twin
/// of [`ShapeFace::uv_turns`](petramond_world::block::ShapeFace). Two bits,
/// split across each word's last free bit: low bit in `packed` bit 31, high
/// bit in `packed2` bit 31 (both documented free/reserved). Read ONLY on
/// `UV_MODE_NONE` non-overlay non-fluid faces — every other UV lane
/// (`CELL_LOCAL` faces, box sets, the log remap) bakes its mapping into the
/// explicit UV it carries, so its turn bits stay zero.
///
/// `block.wgsl` applies it to the tile-local uv in the same order
/// `ShapeFace::texel_uv` does CPU-side (carve, then turn — the greedy span
/// multiply happens BEFORE the turn), and `model3d.wgsl` to the held/dropped/
/// icon cube's tile rect. A row's `uv_rotation` rides wherever the row's tiles
/// go, so a placed block, its held cube and its icon cannot disagree.
pub const UV_TURN_LO_FLAG: u32 = 1 << 31;
pub const UV_TURN_HI_FLAG2: u32 = 1 << 31;

#[inline]
pub fn pack_uv_turn(turn: u32) -> u32 {
    debug_assert!(turn < 4, "uv turn is two bits");
    (turn & 1) << 31
}

#[inline]
pub fn pack_uv_turn2(turn: u32) -> u32 {
    debug_assert!(turn < 4, "uv turn is two bits");
    (turn >> 1) << 31
}

/// GPU vertex for the chunk's bbmodel-block geometry, `pos` in mesh space like
/// [`Vertex`]'s: EXPLICIT attributes
/// (not the packed tile word), because a `.bbmodel` face carries an arbitrary
/// sub-rectangle UV into the model atlas that the tile-packed [`Vertex`] can't express.
/// `shade` is the directional face shade only and `light` carries the cell's
/// (sky, block) light fractions separately, so the world-model shader applies
/// the sim's day/night sky scale at DRAW time — a placed model darkens at night
/// exactly like the terrain around it (a remesh-time bake could not, since
/// meshes don't rebuild when the sun sets).
/// **32 bytes.** It was 44 while the four light fractions rode `[f32;4]`: the
/// sky and the three block channels are 6-bit integers scaled by 1/63, so 160
/// bits carried 24 bits of information. [`pack_model_light`] folds them into
/// one word and the shader divides — the same `f32(k)/63.0` the CPU did, so
/// the packing is bit-for-bit lossless, not a quality trade. `shade` stays a
/// float: it is a baked AO product with no integer form to recover.
///
/// This stream rides the packed terrain columns, so the stride is VRAM, CPU
/// mesh RAM and upload bandwidth on every model block in the world.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
    pub light: u32,
    pub tint: u32,
}

#[inline]
pub fn pack_model_light(sky6: u32, block: BlockLight6) -> u32 {
    let [r, g, b] = block.channels();
    (sky6 & 0x3F) | ((r & 0x3F) << 6) | ((g & 0x3F) << 12) | ((b & 0x3F) << 18)
}

pub const MODEL_TINT_NONE: u32 = 0x00FF_FFFF;

pub use petramond_world::shade::ContactShadowVertex;

/// Terrain geometry stores NO indices: four consecutive vertices are one quad
/// and every terrain draw reads one shared, process-wide quad index buffer
/// (`0,1,2, 0,2,3` repeated) with the section's first vertex as `base_vertex`.
/// That is 24 bytes less per quad on the CPU mesh and in VRAM — 112 MiB of
/// terrain VRAM at render distance 32.
///
/// It holds only because every emitter keeps its corners in canonical order.
/// The opposite ambient-occlusion diagonal is expressed by ROTATING the four
/// corners (same two triangles, same winding), and a quad that must be visible
/// from behind appends a second, reverse-ordered copy of its corners — see
/// [`push_back_face`].
///
/// Translucent fluid TOP faces need neither: they ride a separate stream drawn
/// with culling off.
#[inline]
pub fn push_back_face(vbuf: &mut Vec<Vertex>, start: u32) {
    let s = start as usize;
    let back = [vbuf[s], vbuf[s + 3], vbuf[s + 2], vbuf[s + 1]];
    vbuf.extend_from_slice(&back);
}
