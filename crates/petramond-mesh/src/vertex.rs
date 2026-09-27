mod chunk_mesh;
pub mod transition;
pub mod wgsl;
pub use chunk_mesh::{ChunkMesh, QuadLayer};
use petramond_world::light::BlockLight6;

/// Per-face directional shade factors, mirrored in `block.wgsl`.
pub use petramond_world::shade::SHADES;

/// The CPU block vertex: 24 bytes. A section mesh emits `pos` in MESH space —
/// column-local X/Z, world Y — so no absolute coordinate is ever rounded to
/// `f32`; sealing a mesh ([`ChunkMesh::seal`]) quantizes it into
/// [`TerrainVertex`]. Dynamic bakes (item entities, chests, doors, break
/// overlay) upload it directly.
///
/// `tint` is LINEAR RGB packed unorm8 ([`pack_tint`]; the GPU reads it as
/// `Unorm8x4` — linear values in a linear-interpreted format, so no sRGB OETF
/// level shift). `packed` / `packed2` match the terrain layout so the shared
/// block shader body can shade both.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    /// Linear RGB tint in unorm8 lanes; byte 3 carries the block light's chroma
    /// low byte (see [`BlockLight6::tint_word`]).
    pub tint: u32,
    /// Folded tile + corner + shade + overlay + AO + SKY light. [`pack_vertex`] is
    /// the sole owner of this bit layout (see its doc); the vertex shader decodes
    /// it (selecting uv from the CPU-uploaded `tile_uv()` table — never recomputing
    /// — and light from `SHADES * AO`).
    pub packed: u32,
    /// Second packed word: block light plus the optional cell-local UV. See
    /// `pack_vertex2` and [`pack_cell_uv`], the owners of its bit layout.
    pub packed2: u32,
}

/// Fixed-point scale for [`TerrainVertex::pos`]: one unit = 1/64 block.
/// Water surface Y stays sub-block accurate. NOTE: sub-pixel offsets (like the
/// greedy T-junction overlap) do NOT survive this grid — that overlap is
/// applied in `vs_terrain` (`greedy_overlap_push`), never baked into vertices.
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

/// `x.round() as i32` (half away from zero; NaN is 0; saturating past
/// ±2^30) without the libm call `f32::round` lowers to on the baseline
/// x86-64 target — the vertex quantisers run it for every vertex.
#[inline]
pub(crate) fn round_i32(x: f32) -> i32 {
    const LIMIT: f32 = (1 << 30) as f32;
    let x = x.clamp(-LIMIT, LIMIT);
    let t = x as i32;
    // Exact: `x` and its truncation share an exponent range below 2^30.
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
        let q = |p: f32| {
            round_i32(p * TERRAIN_POS_SCALE).clamp(i16::MIN as i32, i16::MAX as i32) as i16
        };
        Self {
            pos: v.pos.map(q),
            _pad: 0,
            tint: v.tint,
            packed: v.packed,
            packed2: v.packed2,
        }
    }

    /// Inverse of [`from_mesh`](Self::from_mesh) for tests (round-trip within 1/64 block).
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
            // Past the old cap: the id the 8-bit field could not hold.
            (256, 1, 2, 1, 31, false, 1),
            (TILE_MASK, 2, 1, 2, 17, true, OVERLAY_MASK),
        ];
        for (tile, corner, shade, ao, sky, has_overlay, overlay) in cases {
            let packed = pack_vertex(tile, corner, shade, has_overlay, ao, sky);
            let packed2 =
                BlockLight6::grey(45).packed2_bits() | pack_overlay(overlay) | pack_normal_code(5);
            // --- mirror of the WGSL decode ---
            assert_eq!(packed & 0x7FF, tile, "tile");
            assert_eq!((packed >> 11) & 0x3, corner, "corner");
            assert_eq!((packed >> 13) & 0x3, shade, "shade");
            assert_eq!((packed >> 15) & 0x3, ao, "ao");
            assert_eq!((packed >> 17) & 0x3F, sky, "sky");
            assert_eq!((packed >> 26) & 0x1, has_overlay as u32, "has_overlay");
            assert_eq!((packed2 >> 20) & 0x7FF, overlay, "overlay payload");
            assert_eq!(packed2 & 0x3F, 45, "block light");
            assert_eq!((packed2 >> 16) & 0x7, 5, "normal code");
            // The UV mode is OR-ed in above the fields by every emitter, so it
            // must not collide with them.
            let with_mode = packed | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT);
            assert_eq!((with_mode >> 23) & 0x7, UV_MODE_CELL_LOCAL, "uv mode");
            assert_eq!(with_mode & !(0x7 << 23), packed, "uv mode overlaps a field");
        }

        // The two `packed2` lanes the loop above leaves empty, at their extremes:
        // the cell-local UV (0..=16 in 1/16ths, so FIVE bits each — 16 does not
        // fit in four) and the dye-base flag between it and the overlay payload.
        for (u16ths, v16ths) in [(0u32, 0u32), (16, 0), (0, 16), (16, 16), (7, 11)] {
            let packed2 =
                BlockLight6::grey(63).packed2_bits() | pack_cell_uv(u16ths, v16ths) | DYED_FLAG2;
            assert_eq!((packed2 >> 6) & 0x1F, u16ths, "cell-local u");
            assert_eq!((packed2 >> 11) & 0x1F, v16ths, "cell-local v");
            assert_eq!((packed2 >> 19) & 0x1, 1, "dyed flag");
            assert_eq!(packed2 & 0x3F, 63, "block light survives the uv lanes");
            assert_eq!((packed2 >> 20) & 0x7FF, 0, "uv must not reach the overlay");
        }

        // The row-declared UV turn: two bits split across each word's last bit
        // (the WGSL decode is `((packed >> 31) & 1) | (((packed2 >> 31) & 1) << 1)`).
        // They must survive every other tenant of both words and not disturb
        // the overlay payload or the chroma nibble beside them.
        for turn in 0..4u32 {
            // A COLOURED light: grey writes no chroma bits at all, so it could
            // not prove the turn bit sits beside a live chroma nibble.
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

        // --- mirror of the tint word ---
        // `tint` is one `Unorm8x4` attribute: the GPU splits it into four
        // little-endian unorm bytes; the shaders declare it `vec4<f32>`, take
        // lanes 0..3 as the albedo tint and lane 3 as the block-light chroma
        // low byte. So RGB must ride bytes 0/1/2 in that order.
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
            // The dye post-pass must not wipe the chroma the vertex was lit with.
            let lit = BlockLight6::new(63, 12, 40).tint_word(rgb);
            assert_eq!(retint(lit, [0.5, 0.5, 0.5]) >> 24, lit >> 24);
            assert_eq!(retint(lit, rgb), lit);
        }

        // The pipeline binds `tint` as ONE `Unorm8x4` attribute at a literal
        // struct offset (`render::pipeline`), so a reordered field would feed
        // the shader a packed word as a colour.
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
    /// chroma bits (so a white-lit vertex is bit-identical to the pre-colour
    /// engine, and a path that drops the chroma degrades to grey rather than
    /// red), and black has exactly one spelling.
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
                    // Every other tenant of the two words set, so the chroma
                    // lanes must not overlap any of them.
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
                // The neighbouring tenants survive the chroma bits.
                assert_eq!(v.packed & TILE_MASK, TILE_MASK);
                assert_eq!((v.packed >> 26) & 0x1, 1, "has-overlay");
                assert_eq!((v.packed2 >> 20) & OVERLAY_MASK, OVERLAY_MASK);
            }
        }
        // Colourless light is free: no chroma bit anywhere.
        for v in 0..=63u32 {
            let grey = BlockLight6::grey(v);
            assert_eq!(grey.packed_bits(), 0, "grey level {v} spent packed bits");
            assert_eq!(grey.tint_word([1.0; 3]), pack_tint([1.0; 3]));
        }
        assert_eq!(BlockLight6::DARK.packed2_bits(), 0);
    }

    /// The chroma low byte rides the `tint` alpha lane, which the generated
    /// module's lane audit cannot see (it only parses `packed`/`packed2`
    /// reads). Both shaders that decode block light must actually hand it to
    /// `block_light_rgb`, and must declare the attribute wide enough to
    /// receive it.
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

    /// The greedy merge span shares the overlay payload, and `block.wgsl` reads
    /// its two nibbles at fixed shifts (`packed2 >> 20` and `>> 24`). Mirror
    /// that decode and round-trip both extremes: an off-by-one word or shift
    /// here is a stretched-texture bug nothing else catches.
    #[test]
    fn the_greedy_span_round_trips_through_the_shaders_decode() {
        for (w, h) in [(1u32, 1u32), (16, 1), (1, 16), (16, 16), (5, 11)] {
            let packed2 =
                BlockLight6::grey(9).packed2_bits() | pack_overlay(pack_greedy_span(w, h));
            assert_eq!(unpack_greedy_span(packed2), (w, h));
            // --- mirror of the WGSL decode ---
            assert_eq!(((packed2 >> 20) & 0xF) + 1, w, "gw");
            assert_eq!(((packed2 >> 24) & 0xF) + 1, h, "gh");
            // A 1×1 face must leave the payload zero: `greedy_overlap_push`
            // gates the T-junction nudge on a NONZERO payload.
            assert_eq!(
                (packed2 >> OVERLAY_SHIFT2) & OVERLAY_MASK == 0,
                (w, h) == (1, 1),
            );
        }
    }

    /// The tile field must span exactly the shared id-space ceiling
    /// (`tile::MAX_TILES`) the atlas cap and the shader uv-rect table also
    /// derive from — the drift that would silently truncate tile ids.
    #[test]
    fn the_tile_field_spans_the_shared_tile_id_space() {
        assert_eq!(MAX_TILES, TILE_MASK as usize + 1);
    }
}

/// The `Vertex::tint` ALPHA lane, bits 24..32. The GPU reads `tint` as
/// `Unorm8x4` — four bytes of stride the vertex already pays for — while the
/// three colour lanes below it carry the albedo tint. It holds the low byte of
/// the block light's CHROMA word (see [`BlockLight6::tint_word`]); it is the
/// only free space in the vertex that costs nothing to use.
pub const TINT_ALPHA_SHIFT: u32 = 24;

/// Pack a linear RGB tint into the `Vertex::tint` unorm8 word, little-endian
/// `r | g<<8 | b<<16`, matching `VertexFormat::Unorm8x4`'s lane order (each
/// channel is `0..=1` — biome and dye tints never exceed 1). The SINGLE owner
/// of the tint encoding.
///
/// The alpha lane is left ZERO, which is exactly "colourless block light" (see
/// [`BlockLight6::tint_word`], which is what a LIT vertex builds its tint with)
/// — so an unlit or white-lit vertex needs no chroma bits at all.
#[inline]
pub fn pack_tint(rgb: [f32; 3]) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    q(rgb[0]) | (q(rgb[1]) << 8) | (q(rgb[2]) << 16)
}

/// Replace the colour lanes of an existing `Vertex::tint` word, keeping its
/// chroma (alpha) lane. Every CPU path that post-processes an already-built
/// vertex's tint (the dye multiply on a held/dropped/icon block) must go
/// through this: a plain `unpack_tint` → `pack_tint` round trip would erase
/// the light colour the vertex was baked with.
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
/// white vertex writes no chroma bits at all (its `packed` word is bit-identical
/// to the pre-colour engine's), black has one spelling, and a path that carries
/// only part of the split degrades to colourless light rather than a red cast.
pub const CHROMA_HI_SHIFT: u32 = 27;
pub const CHROMA_HI_MASK: u32 = 0xF;
const CHROMA_LO_BITS: u32 = 8;

/// Vertex-format packing over [`BlockLight6`] — presentation bit layout owned
/// by the mesher (the light value itself lives in the world crate).
pub trait BlockLightVertexExt: Copy {
    fn chroma(self) -> u32;
    fn packed_bits(self) -> u32;
    fn packed2_bits(self) -> u32;
    fn tint_word(self, rgb: [f32; 3]) -> u32;
}

impl BlockLightVertexExt for BlockLight6 {
    /// The 12-bit chroma word: `(g ^ r) | (b ^ r) << 6`.
    #[inline]
    fn chroma(self) -> u32 {
        (self.g() ^ self.r()) | ((self.b() ^ self.r()) << 6)
    }

    /// Bits to OR into `Vertex::packed` (chroma high nibble).
    #[inline]
    fn packed_bits(self) -> u32 {
        ((self.chroma() >> CHROMA_LO_BITS) & CHROMA_HI_MASK) << CHROMA_HI_SHIFT
    }

    /// Bits to OR into `Vertex::packed2` (the RED channel, bits 0..6 — the lane
    /// block light has always used, so grey light is unchanged there).
    #[inline]
    fn packed2_bits(self) -> u32 {
        self.r() & BLOCK_LIGHT_MASK
    }

    /// The complete `Vertex::tint` word for a lit vertex: albedo tint in the
    /// three colour lanes, chroma low byte in the alpha lane.
    #[inline]
    fn tint_word(self, rgb: [f32; 3]) -> u32 {
        pack_tint(rgb) | ((self.chroma() & 0xFF) << TINT_ALPHA_SHIFT)
    }
}

/// The Rust mirror of the shaders' block-light decode: reassemble the three
/// channels from the three words they are split across. Foliage subdivision
/// uses this to interpolate light without blending the encoded chroma bits.
#[inline]
pub fn decode_vertex_light(v: &Vertex) -> BlockLight6 {
    let chroma = ((v.tint >> TINT_ALPHA_SHIFT) & 0xFF)
        | (((v.packed >> CHROMA_HI_SHIFT) & CHROMA_HI_MASK) << CHROMA_LO_BITS);
    let r = v.packed2 & BLOCK_LIGHT_MASK;
    BlockLight6::new(r, (chroma & 0x3F) ^ r, ((chroma >> 6) & 0x3F) ^ r)
}

/// Inverse of [`pack_tint`] for the rare CPU path that post-processes an
/// already-built vertex. Prefer [`retint`] when the word is a LIT vertex's:
/// this drops the chroma lane.
#[inline]
pub fn unpack_tint(tint: u32) -> [f32; 3] {
    [
        (tint & 0xFF) as f32 / 255.0,
        ((tint >> 8) & 0xFF) as f32 / 255.0,
        ((tint >> 16) & 0xFF) as f32 / 255.0,
    ]
}

/// Fold one vertex's attributes into the packed `u32` word — the SINGLE owner of
/// the `Vertex::packed` bit layout. Everything that emits a mesh vertex (the chunk
/// mesher's cube faces and cross-plants; `render::item_cube` mirrors the same
/// field meanings) routes through here, so the layout is defined in exactly one
/// place.
///
/// Bit layout — the constants below are the ONE definition; the shaders decode
/// it through the WGSL module [`wgsl::layout`] generates from them:
///   0..11 tile id | 11..13 corner (0..3) | 13..15 shade index (into `SHADES`)
///   15..17 AO (0 dark..3 bright) | 17..23 SKYLIGHT ONLY (0 dark..63 full sky)
///   23..26 UV mode | 26 has-overlay flag | 27..31 block-light chroma high
///   nibble ([`CHROMA_HI_SHIFT`]) | 31 uv-turn low bit ([`pack_uv_turn`])
///
/// Block light moved to `packed2` bits 0..6 + the chroma split (see
/// [`BlockLight6`]) so the shader can dim the sky term (day/night mods) without
/// dimming torch light. The overlay PAYLOAD lives in `packed2` too (see
/// [`pack_overlay`]) — it is itself a tile id, so it had to widen with the tile
/// field and no longer fits beside it.
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

/// Width of the `packed` tile-id field. 11 bits addresses 2048 atlas tiles —
/// the cap the tile catalogue enforces at pack load (`tile::MAX_TILES`, with
/// an error naming the count). An OVERLAY tile id is the same currency and
/// gets the same width in `packed2`. The shaders' masks derive from this
/// (see [`wgsl`]), so widening it is this edit plus finding the bits.
pub const TILE_BITS: u32 = petramond_world::tile::MAX_TILES.trailing_zeros();
pub const TILE_MASK: u32 = (1 << TILE_BITS) - 1;
/// How many atlas tiles the vertex format can address — the ONE definition the
/// catalogue's load-time cap and the shader tile masks both derive from.
pub use petramond_world::tile::MAX_TILES;
pub const CORNER_SHIFT: u32 = 11;
pub const SHADE_SHIFT: u32 = 13;
pub const AO_SHIFT: u32 = 15;
pub const SKY_SHIFT: u32 = 17;

/// `Vertex::packed` bit 26. In the chunk pass it means "composite the overlay
/// payload"; the model3d pass, which never composites overlays, reuses the same
/// bit as the model3d solid-colour sentinel.
pub const OVERLAY_FLAG: u32 = 1 << 26;

/// The overlay payload's home in `packed2`, bits 20..31.
///
/// Three meanings share it, exactly as they shared the old `packed` 12..20: a
/// grass SIDE carries its overlay TILE ID (with `has_overlay` set); a greedy
/// quad carries its merged span as `(w - 1) | (h - 1) << 4`; a flowing-water TOP
/// carries its quantized flow heading. All three are read only by the pass that
/// wrote them.
pub const OVERLAY_SHIFT2: u32 = 20;
pub const OVERLAY_MASK: u32 = (1 << TILE_BITS) - 1;

/// Fold an overlay payload into `Vertex::packed2` — see [`OVERLAY_SHIFT2`].
#[inline]
pub fn pack_overlay(payload: u32) -> u32 {
    debug_assert!(payload <= OVERLAY_MASK, "overlay payload exceeds its field");
    (payload & OVERLAY_MASK) << OVERLAY_SHIFT2
}

/// A greedy-merged quad's span as an overlay payload: `(w - 1) | (h - 1) << 4`,
/// each 4 bits (a merge never exceeds one 16-cell section axis). The shader
/// multiplies the corner uv by it so one tile REPEATs across the merge.
///
/// Paired with `unpack_greedy_span` so the emitter, the tests and the WGSL
/// mirror all describe one layout — spelling the two nibbles out by hand is
/// how the height read silently drifted onto the AO/sky bits when the payload
/// moved words.
#[inline]
pub fn pack_greedy_span(w: u32, h: u32) -> u32 {
    debug_assert!(
        (1..=16).contains(&w) && (1..=16).contains(&h),
        "span 1..=16"
    );
    ((w - 1) & 0xF) | (((h - 1) & 0xF) << 4)
}

/// The `(w, h)` a greedy quad's `packed2` word carries — the exact read
/// `block.wgsl` performs. Nothing in the engine decodes this (the GPU does),
/// so it exists as the encoder's inverse for the tests that pin the layout.
#[cfg(test)]
#[inline]
pub fn unpack_greedy_span(packed2: u32) -> (u32, u32) {
    let payload = (packed2 >> OVERLAY_SHIFT2) & OVERLAY_MASK;
    ((payload & 0xF) + 1, ((payload >> 4) & 0xF) + 1)
}

/// The `Vertex::packed2` bit layout — owned here together with
/// [`pack_cell_uv`], [`pack_overlay`] and [`pack_normal_code`] (decoded in
/// WGSL through the generated [`wgsl::layout`] module):
///
///   0..6 block light RED ([`BlockLight6::packed2_bits`]; green and blue ride
///        the chroma split — see [`CHROMA_HI_SHIFT`])
///   | 6..16 cell-local uv ([`pack_cell_uv`], read only in [`UV_MODE_CELL_LOCAL`])
///        OR, on a [`UV_MODE_NONE`] fluid face, its medium ([`pack_fluid_face`])
///   | 16..19 face-normal code ([`pack_normal_code`])
///   | 19 dyed flag ([`DYED_FLAG2`])
///   | 20..31 overlay payload ([`pack_overlay`]) | 31 uv-turn high bit
///   ([`pack_uv_turn2`])
///
/// Each block channel is 6 bits like the sky channel so the shader's per-channel
/// `block_term` mirrors the sky curve exactly.
///
/// Width of the block-light lane at bits 0..6.
pub const BLOCK_LIGHT_MASK: u32 = 0x3F;

/// `Vertex::packed2` bit 19: the vertex samples its tile's DYE-BASE twin
/// (desaturated, brightness-normalized — see `atlas`) instead of the base
/// tile. Set by every emitter whose face carries a `petramond:tint` multiply,
/// so the tint lands on a peak-white base and can both dye and whiten.
/// `block.wgsl` resolves it as `layer + tile count` on the terrain texture
/// array; `model3d.wgsl` as `v + 0.5` on the composed 2D atlas.
pub const DYED_FLAG2: u32 = 1 << 19;

/// Face-normal code, packed into `Vertex::packed2` bits 16..19: 0 = neutral (no
/// world-space face direction — the shader keeps the classic `SHADES` shading),
/// 1..=6 = [`super::face::Face::normal_code`] for sun-directional N·L shading in
/// `block.wgsl`.
#[inline]
pub fn pack_normal_code(code: u32) -> u32 {
    (code & NORMAL_CODE_MASK) << NORMAL_CODE_SHIFT
}

pub const NORMAL_CODE_SHIFT: u32 = 16;
pub const NORMAL_CODE_MASK: u32 = 0x7;

/// Explicit tile-local UV in 1/16ths (0..=16), packed into `Vertex::packed2`
/// bits 6..11 (u) and 11..16 (v). Shaders read it only when the vertex's UV mode
/// is [`UV_MODE_CELL_LOCAL`]; partial faces (stairs) use it to sample the
/// sub-rectangle of their tile matching the quad's position inside the cell, so
/// the shape textures as a full block with a chunk cut out.
#[inline]
pub fn pack_cell_uv(u16ths: u32, v16ths: u32) -> u32 {
    debug_assert!(u16ths <= 16 && v16ths <= 16);
    ((u16ths & CELL_UV_MASK) << CELL_UV_U_SHIFT) | ((v16ths & CELL_UV_MASK) << CELL_UV_V_SHIFT)
}

/// The two cell-local UV lanes in `packed2`. Five bits each because the value
/// is 1/16ths INCLUSIVE of 16 (a face flush with the far cell edge).
pub const CELL_UV_U_SHIFT: u32 = 6;
pub const CELL_UV_V_SHIFT: u32 = 11;
pub const CELL_UV_MASK: u32 = 0x1F;

/// A fluid face's medium in `packed2`: `medium index + 1` at bits 6..15 (0 is
/// "not a fluid face"), plus [`FLUID_FLOW_FLAG2`] when the face shows the
/// fluid's flow strip. It shares the cell-local uv lane — a fluid face is
/// always [`UV_MODE_NONE`], so the two tenants never meet on one vertex.
#[inline]
pub fn pack_fluid_face(medium: u32, flow: bool) -> u32 {
    debug_assert!(medium < MAX_FLUID_MEDIA, "fluid medium exceeds its lane");
    (((medium + 1) & FLUID_MEDIUM_MASK) << FLUID_MEDIUM_SHIFT)
        | if flow { FLUID_FLOW_FLAG2 } else { 0 }
}

pub const FLUID_MEDIUM_SHIFT: u32 = CELL_UV_U_SHIFT;
pub const FLUID_MEDIUM_MASK: u32 = 0x1FF;
/// `packed2` bit 15 on a fluid face: its top turns toward the flow heading and
/// its side crops to the fluid's height.
pub const FLUID_FLOW_FLAG2: u32 = 1 << 15;
/// How many fluid media the vertex lane can address.
pub const MAX_FLUID_MEDIA: u32 = FLUID_MEDIUM_MASK;

/// Packed UV mode field, shared by `block.wgsl` and dynamic block geometry.
pub const UV_MODE_SHIFT: u32 = 23;
pub const UV_MODE_NONE: u32 = 0;
pub const UV_MODE_THIN_U: u32 = 1;
pub const UV_MODE_THIN_V: u32 = 2;
/// The vertex carries an explicit tile-local UV in `packed2` (see [`pack_cell_uv`]).
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

/// The low bit of a face's UV turn, OR-ed into `Vertex::packed` (bit 31).
#[inline]
pub fn pack_uv_turn(turn: u32) -> u32 {
    debug_assert!(turn < 4, "uv turn is two bits");
    (turn & 1) << 31
}

/// The high bit of a face's UV turn, OR-ed into `Vertex::packed2` (bit 31).
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
    /// `(sky, block_r, block_g, block_b)` as four 6-bit levels — see
    /// [`pack_model_light`]. The block channel is per-colour so a placed model
    /// sits in coloured light like the terrain around it. Bit 31 marks unlit artwork.
    pub light: u32,
    /// Multiply colour and animation slot packed as `0xAARRGGBB`; `0xFFFFFF` (white) for every
    /// vertex of a row that declares no `tint_parts`, which is almost all of
    /// them. Packed rather than three floats because this stream is sparse but
    /// not free — a model block pays it per vertex.
    pub tint: u32,
}

/// The four 6-bit light levels of a [`ModelVertex`], `sky | r<<6 | g<<12 |
/// b<<18`. The sole owner of that layout; `mob.wgsl`'s `vs_world_model`
/// mirrors the decode by hand and divides each lane by 63.
#[inline]
pub fn pack_model_light(sky6: u32, block: BlockLight6) -> u32 {
    let [r, g, b] = block.channels();
    (sky6 & 0x3F) | ((r & 0x3F) << 6) | ((g & 0x3F) << 12) | ((b & 0x3F) << 18)
}

/// The untinted `ModelVertex::tint` — white, i.e. the texture unmodified.
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
