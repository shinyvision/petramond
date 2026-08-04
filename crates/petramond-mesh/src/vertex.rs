use petramond_world::light::BlockLight6;

/// Per-face directional shade factors, mirrored in `block.wgsl`.
pub use petramond_world::shade::SHADES;

/// GPU vertex for dynamic bakes (item entities, chests, doors, break overlay):
/// 24 bytes with absolute world `pos` as `f32`. Terrain packed columns use
/// [`TerrainVertex`] instead (column-local fixed-point `pos`).
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
    /// [`pack_vertex2`] and [`pack_cell_uv`], the owners of its bit layout.
    pub packed2: u32,
}

/// Fixed-point scale for [`TerrainVertex::pos`]: one unit = 1/64 block.
/// Water surface Y stays sub-block accurate. NOTE: sub-pixel offsets (like the
/// greedy T-junction overlap) do NOT survive this grid — that overlap is
/// applied in `vs_terrain` (`greedy_overlap_push`), never baked into vertices.
pub const TERRAIN_POS_SCALE: f32 = 64.0;

/// Packed-column terrain vertex: **20 bytes**. `pos` is column-local XZ + world Y
/// in [`TERRAIN_POS_SCALE`] fixed point (`i16`); the draw binds the column's
/// world XZ origin as an instance-step attribute and the terrain VS reconstructs
/// absolute world position. CPU meshes still use [`Vertex`]; conversion happens
/// at upload / patch time.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainVertex {
    pub pos: [i16; 3],
    pub _pad: i16,
    pub tint: u32,
    pub packed: u32,
    pub packed2: u32,
}

impl TerrainVertex {
    #[inline]
    pub fn from_world(v: &Vertex, col_ox: i32, col_oz: i32) -> Self {
        let q = |world: f32, origin: f32| {
            ((world - origin) * TERRAIN_POS_SCALE)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16
        };
        Self {
            pos: [
                q(v.pos[0], col_ox as f32),
                q(v.pos[1], 0.0),
                q(v.pos[2], col_oz as f32),
            ],
            _pad: 0,
            tint: v.tint,
            packed: v.packed,
            packed2: v.packed2,
        }
    }

    /// Inverse of [`from_world`] for tests (round-trip within 1/64 block).
    #[cfg(test)]
    pub fn to_world(self, col_ox: i32, col_oz: i32) -> [f32; 3] {
        [
            self.pos[0] as f32 / TERRAIN_POS_SCALE + col_ox as f32,
            self.pos[1] as f32 / TERRAIN_POS_SCALE,
            self.pos[2] as f32 / TERRAIN_POS_SCALE + col_oz as f32,
        ]
    }
}

#[cfg(test)]
mod terrain_vertex_tests {
    use super::*;

    #[test]
    fn terrain_pos_quantizes_within_half_unit() {
        let v = Vertex {
            pos: [16.0 + 3.125, 64.5, -32.0 + 0.0625],
            tint: 0xFF00_00FF,
            packed: 1,
            packed2: 2,
        };
        let t = TerrainVertex::from_world(&v, 16, -32);
        let back = t.to_world(16, -32);
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

    /// The three GPU words are decoded by HAND-MIRRORED WGSL (`block.wgsl`,
    /// `model3d.wgsl`, `break_overlay.wgsl`), which no Rust test can execute —
    /// a shift that drifts between the two is invisible until something renders
    /// wrong. So this mirrors the shaders' decode of BOTH packed words AND the
    /// `Unorm8x4` tint word (alpha lane included) and round-trips every field at
    /// its extremes, including a tile id ABOVE the old 8-bit ceiling (the whole
    /// point of the widening) and the overlay payload in its new home.
    ///
    /// The literals below are the SHADERS' literals, deliberately spelled out
    /// rather than derived from the constants: moving a Rust shift must break
    /// this test, because the WGSL it mirrors did not move with it. If you move
    /// a field, update this decode to match the shader you edited.
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

    /// The Rust mirror above proves the encoders agree with a decode SPELLED IN
    /// RUST. This proves the decode spelled in WGSL is the same one: re-read the
    /// three shader sources and collect every `packed`/`packed2` bit extraction
    /// they perform, then require each to name a lane the Rust side defines.
    ///
    /// A new lane carved out of the free bits shows up here as an unknown
    /// `(shift, mask)` until it is listed, and a lane that MOVES on the Rust
    /// side leaves a shader decoding a shift no longer in the table. Either way
    /// this fails before anything renders.
    #[test]
    fn every_shader_decodes_the_packed_words_at_the_rust_lanes() {
        // (word, shift, mask, what it is) — the complete Rust-side lane map.
        // `packed2` bits 20..32 hold three mutually exclusive tenants, so the
        // overlay payload appears at three widths (a whole tile id, the two
        // greedy-span nibbles, and both nibbles read at once by the T-junction
        // nudge's "payload is nonzero" gate).
        let lanes: &[(&str, u32, u32, &str)] = &[
            ("packed", 0, TILE_MASK, "tile id"),
            ("packed", CORNER_SHIFT, 0x3, "corner"),
            ("packed", SHADE_SHIFT, 0x3, "shade index"),
            ("packed", AO_SHIFT, 0x3, "ao"),
            ("packed", SKY_SHIFT, 0x3F, "skylight"),
            ("packed", UV_MODE_SHIFT, 0x7, "uv mode"),
            ("packed", OVERLAY_FLAG.trailing_zeros(), 0x1, "has-overlay"),
            (
                "packed",
                CHROMA_HI_SHIFT,
                CHROMA_HI_MASK,
                "chroma high nibble",
            ),
            ("packed2", 0, BLOCK_LIGHT_MASK, "block light red"),
            ("packed2", CELL_UV_U_SHIFT, CELL_UV_MASK, "cell-local u"),
            ("packed2", CELL_UV_V_SHIFT, CELL_UV_MASK, "cell-local v"),
            (
                "packed2",
                NORMAL_CODE_SHIFT,
                NORMAL_CODE_MASK,
                "normal code",
            ),
            ("packed2", DYED_FLAG2.trailing_zeros(), 0x1, "dyed flag"),
            ("packed2", OVERLAY_SHIFT2, OVERLAY_MASK, "overlay tile"),
            ("packed2", OVERLAY_SHIFT2, 0xF, "greedy width"),
            ("packed2", OVERLAY_SHIFT2 + 4, 0xF, "greedy height"),
            (
                "packed2",
                OVERLAY_SHIFT2,
                0xFF,
                "greedy span (both nibbles)",
            ),
        ];
        let sources = [
            ("block.wgsl", include_str!("../../petramond-render/shaders/block.wgsl")),
            ("model3d.wgsl", include_str!("../../petramond-render/shaders/model3d.wgsl")),
            (
                "break_overlay.wgsl",
                include_str!("../../petramond-render/shaders/break_overlay.wgsl"),
            ),
        ];
        let mut seen_in_block = std::collections::HashSet::new();
        for (name, src) in sources {
            for (word, shift, mask) in shader_bit_reads(src) {
                assert!(
                    lanes
                        .iter()
                        .any(|&(w, s, m, _)| w == word && s == shift && m == mask),
                    "{name} decodes `{word}` at shift {shift} mask {mask:#X}, \
                     which is not a lane mesh::vertex defines"
                );
                if name == "block.wgsl" {
                    seen_in_block.insert((word, shift, mask));
                }
            }
        }
        // block.wgsl is the full-fat consumer: it reads every lane. One going
        // missing there is a lane silently dropped from the terrain render.
        for &(word, shift, mask, what) in lanes {
            assert!(
                seen_in_block.contains(&(word, shift, mask)),
                "block.wgsl no longer decodes the {what} lane ({word} >> {shift} & {mask:#X})"
            );
        }
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

    /// The chroma low byte rides the `tint` alpha lane, which the lane-shift
    /// audit above cannot see (it only parses `packed`/`packed2` reads). Both
    /// shaders that decode block light must actually read it, and must declare
    /// the attribute wide enough to receive it.
    #[test]
    fn the_light_decoding_shaders_read_the_tint_alpha_lane() {
        for (name, src) in [
            ("block.wgsl", include_str!("../../petramond-render/shaders/block.wgsl")),
            ("model3d.wgsl", include_str!("../../petramond-render/shaders/model3d.wgsl")),
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

    /// Every `(<word> >> Nu) & 0xMu` / `<word> & 0xMu` extraction a WGSL source
    /// performs on the two packed vertex words. Prose mentioning `packed2` is
    /// skipped: only an immediately following shift-or-mask parses.
    fn shader_bit_reads(src: &str) -> Vec<(&'static str, u32, u32)> {
        let hex = |s: &str| -> Option<u32> {
            let end = s.find('u')?;
            u32::from_str_radix(&s[..end], 16).ok()
        };
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(i) = src[at..].find("packed") {
            let start = at + i;
            at = start + "packed".len();
            let word = if src[at..].starts_with('2') {
                at += 1;
                "packed2"
            } else {
                "packed"
            };
            let tail = src[at..].trim_start();
            if let Some(rest) = tail.strip_prefix(">> ") {
                let Some(u) = rest.find("u) & 0x") else {
                    continue;
                };
                let (Ok(shift), Some(mask)) = (rest[..u].parse::<u32>(), hex(&rest[u + 7..]))
                else {
                    continue;
                };
                out.push((word, shift, mask));
            } else if let Some(rest) = tail.strip_prefix("& 0x") {
                if let Some(mask) = hex(rest) {
                    out.push((word, 0, mask));
                }
            }
        }
        out
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
/// channels from the three words they are split across. The engine itself never
/// needs this (the GPU does the decode) — it exists so tests can prove an
/// emitter's colour survives the split, which is the one thing a hand-mirrored
/// WGSL decode cannot check for itself.
#[cfg(any(test, feature = "test-support"))]
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
/// Bit layout — the constants below are the ONE definition, mirrored by hand in
/// `src/shaders/block.wgsl`, `model3d.wgsl` and `break_overlay.wgsl`:
///   0..11 tile id | 11..13 corner (0..3) | 13..15 shade index (into `SHADES`)
///   15..17 AO (0 dark..3 bright) | 17..23 SKYLIGHT ONLY (0 dark..63 full sky)
///   23..26 UV mode | 26 has-overlay flag | 27..31 block-light chroma high
///   nibble ([`CHROMA_HI_SHIFT`]) | 31 free
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
/// the cap `atlas::build` enforces and `render::uniforms::UV_RECTS_LEN` sizes
/// its table to. An OVERLAY tile id is the same currency and gets the same
/// width in `packed2`.
pub const TILE_BITS: u32 = petramond_world::tile::MAX_TILES.trailing_zeros();
pub const TILE_MASK: u32 = (1 << TILE_BITS) - 1;
/// How many atlas tiles the vertex format can address — the ONE definition the
/// atlas loader's cap and the shader uv-rect table both derive from, so the
/// three cannot drift.
pub use petramond_world::tile::MAX_TILES;
pub const CORNER_SHIFT: u32 = 11;
pub const SHADE_SHIFT: u32 = 13;
pub const AO_SHIFT: u32 = 15;
pub const SKY_SHIFT: u32 = 17;

/// `Vertex::packed` bit 26. In the chunk pass it means "composite the overlay
/// payload"; the model3d pass, which never composites overlays, reuses the same
/// bit as [`SOLID_COLOR_FLAG`](crate::render::SOLID_COLOR_FLAG).
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
/// Paired with [`unpack_greedy_span`] so the emitter, the tests and the WGSL
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
/// [`pack_cell_uv`], [`pack_overlay`] and [`pack_normal_code`] (all mirrored by
/// hand in `block.wgsl` and `model3d.wgsl`):
///
///   0..6 block light RED ([`BlockLight6::packed2_bits`]; green and blue ride
///        the chroma split — see [`CHROMA_HI_SHIFT`])
///   | 6..16 cell-local uv ([`pack_cell_uv`], read only in [`UV_MODE_CELL_LOCAL`])
///   | 16..19 face-normal code ([`pack_normal_code`])
///   | 19 dyed flag ([`DYED_FLAG2`])
///   | 20..31 overlay payload ([`pack_overlay`]) | 31 RESERVED (zero)
///
/// Each block channel is 6 bits like the sky channel so the shader's per-channel
/// `block_term` mirrors the sky curve exactly; bit 31 is reserved for future
/// per-vertex data and MUST stay zero until a new owner is documented here.
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

/// Packed UV mode field, shared by `block.wgsl` and dynamic block geometry.
pub const UV_MODE_SHIFT: u32 = 23;
pub const UV_MODE_NONE: u32 = 0;
pub const UV_MODE_THIN_U: u32 = 1;
pub const UV_MODE_THIN_V: u32 = 2;
/// The vertex carries an explicit tile-local UV in `packed2` (see [`pack_cell_uv`]).
pub const UV_MODE_CELL_LOCAL: u32 = 3;

/// GPU vertex for the chunk's bbmodel-block geometry: EXPLICIT attributes
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
    /// sits in coloured light like the terrain around it.
    pub light: u32,
    /// Multiply colour packed as `0x00RRGGBB`; `0xFFFFFF` (white) for every
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
/// Water TOP faces are the one exception that needs neither: they ride a
/// separate stream drawn with culling off.
#[inline]
pub fn push_back_face(vbuf: &mut Vec<Vertex>, start: u32) {
    let s = start as usize;
    let back = [vbuf[s], vbuf[s + 3], vbuf[s + 2], vbuf[s + 1]];
    vbuf.extend_from_slice(&back);
}

pub struct ChunkMesh {
    /// Opaque terrain quads, triangulation implied (see [`QuadIdx`]).
    pub opaque: Vec<Vertex>,
    /// WATER geometry: alpha-blended, depth-READ-only (water must not occlude
    /// the terrain behind it), drawn last, farthest section first. Back-face
    /// culled: an exposed side face over shallower water must not show its
    /// back as a dark sheet from the water side.
    pub transparent: Vec<Vertex>,
    /// Water TOP faces, drawn by the same pass with culling OFF so the surface
    /// stays visible from underneath. They used to be a second index winding
    /// over the same vertices; a separate cull-none draw is the index-free
    /// equivalent and rasterizes half the triangles.
    pub transparent_two_sided: Vec<Vertex>,
    /// TRANSLUCENT-BLOCK geometry (ice): alpha-blended but depth-WRITING and
    /// drawn between opaque and water — a 3D sheet of translucent cubes needs
    /// depth to resolve its own face order (buffer order is arbitrary within
    /// a section), which water's read-only convention cannot give it.
    pub translucent: Vec<Vertex>,
    /// Optional opaque LOD used for far chunks. This keeps the normal mesh
    /// byte-identical nearby while allowing far foliage to cull leaf-to-leaf
    /// internals once texture mips make the cutouts read as a dense canopy.
    pub far_opaque: Vec<Vertex>,
    /// bbmodel-block geometry (explicit-UV [`ModelVertex`], sampling the model atlas),
    /// drawn in the renderer's dedicated model pass. Baked here at remesh like the rest
    /// of the chunk; empty for the common chunk with no bbmodel blocks.
    pub model: Vec<ModelVertex>,
    pub model_idx: Vec<u32>,
    /// The alpha-BLEND model faces (semi-transparent texels, routed at template-bake
    /// time): indices into the SAME `model` vertex buffer, drawn by the model-blend
    /// pass after the translucent-block pass. Kept as a second index stream so the
    /// opaque pass never touches a blended triangle.
    pub model_blend_idx: Vec<u32>,
    /// Model→terrain contact-shadow triangles (non-indexed, see
    /// [`ContactShadowVertex`]), drawn by the renderer's dedicated contact pass.
    /// A section can hold contact triangles with an EMPTY model stream (a
    /// multi-cell model's spanning cuboids may all render from a sibling cell),
    /// so contact presence is tracked independently of `model_idx`.
    pub contact: Vec<ContactShadowVertex>,
    /// True until GPU upload has happened. Set by the mesh builder, cleared by
    /// renderer after a successful upload so we don't re-upload every frame.
    pub mesh_dirty: bool,
    /// True once the CPU vertex/index buffers were released after a settled GPU
    /// upload (the geometry then lives only in the packed column buffer). A column
    /// repack cannot read a released mesh; it must force a remesh first.
    pub(in crate) released: bool,
    /// `is_empty()` captured at release time, so emptiness queries stay truthful
    /// after the buffers are gone.
    pub(in crate) released_empty: bool,
}

impl Default for ChunkMesh {
    fn default() -> Self {
        Self::empty()
    }
}

impl ChunkMesh {
    pub fn empty() -> Self {
        Self {
            opaque: vec![],
            transparent: vec![],
            transparent_two_sided: vec![],
            translucent: vec![],
            far_opaque: vec![],
            model: vec![],
            model_idx: vec![],
            model_blend_idx: vec![],
            contact: vec![],
            mesh_dirty: false,
            released: false,
            released_empty: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        if self.released {
            return self.released_empty;
        }
        // A chunk holding ONLY a bbmodel block (empty packed buffers) is NOT empty —
        // its geometry lives in the model stream, which must still upload + draw.
        self.opaque.is_empty()
            && self.transparent.is_empty()
            && self.transparent_two_sided.is_empty()
            && self.translucent.is_empty()
            && self.model_idx.is_empty()
            && self.model_blend_idx.is_empty()
            && self.contact.is_empty()
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    /// Per-stream used bytes of the retained CPU buffers: `(opaque v, opaque i,
    /// far v, far i, transparent v, transparent i, translucent v, translucent i,
    /// model v, model i, contact v)`. For the memory census.
    pub fn stream_bytes(&self) -> [u64; 11] {
        const V: usize = std::mem::size_of::<Vertex>();
        const M: usize = std::mem::size_of::<ModelVertex>();
        const C: usize = std::mem::size_of::<ContactShadowVertex>();
        [
            (self.opaque.len() * V) as u64,
            0,
            (self.far_opaque.len() * V) as u64,
            0,
            ((self.transparent.len() + self.transparent_two_sided.len()) * V) as u64,
            0,
            (self.translucent.len() * V) as u64,
            0,
            (self.model.len() * M) as u64,
            ((self.model_idx.len() + self.model_blend_idx.len()) * 4) as u64,
            (self.contact.len() * C) as u64,
        ]
    }

    /// `(used bytes, allocated-capacity bytes)` of the retained CPU buffers,
    /// for the memory census.
    pub fn memory_bytes(&self) -> (u64, u64) {
        const V: usize = std::mem::size_of::<Vertex>();
        const M: usize = std::mem::size_of::<ModelVertex>();
        const C: usize = std::mem::size_of::<ContactShadowVertex>();
        let used = self.opaque.len() * V
            + self.transparent.len() * V
            + self.transparent_two_sided.len() * V
            + self.translucent.len() * V
            + self.far_opaque.len() * V
            + self.model.len() * M
            + self.contact.len() * C
            + self.model_idx.len() * 4
            + self.model_blend_idx.len() * 4;
        let cap = self.opaque.capacity() * V
            + self.transparent.capacity() * V
            + self.transparent_two_sided.capacity() * V
            + self.translucent.capacity() * V
            + self.far_opaque.capacity() * V
            + self.model.capacity() * M
            + self.contact.capacity() * C
            + self.model_idx.capacity() * 4
            + self.model_blend_idx.capacity() * 4;
        (used as u64, (cap + std::mem::size_of::<Self>()) as u64)
    }

    /// Free the CPU-side geometry of an uploaded mesh. `Vec::new()` (not `clear`)
    /// so the heap allocations are returned, not kept as capacity.
    pub fn release_cpu_buffers(&mut self) {
        debug_assert!(!self.mesh_dirty, "releasing a mesh that was never uploaded");
        self.released_empty = self.is_empty();
        self.released = true;
        self.opaque = Vec::new();
        self.transparent = Vec::new();
        self.transparent_two_sided = Vec::new();
        self.translucent = Vec::new();
        self.far_opaque = Vec::new();
        self.model = Vec::new();
        self.model_idx = Vec::new();
        self.model_blend_idx = Vec::new();
        self.contact = Vec::new();
    }
}
