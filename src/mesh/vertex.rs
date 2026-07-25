/// Per-face directional shade factors, mirrored in `block.wgsl`.
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

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
    /// Linear RGB tint in unorm8 lanes (byte 3 unused, kept 0xFF); see [`pack_tint`].
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
    pub fn to_world(&self, col_ox: i32, col_oz: i32) -> [f32; 3] {
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

    /// The packed words are decoded by HAND-MIRRORED WGSL (`block.wgsl`,
    /// `model3d.wgsl`, `break_overlay.wgsl`), which no Rust test can execute —
    /// a shift that drifts between the two is invisible until something renders
    /// wrong. So this mirrors the shaders' decode and round-trips every field
    /// at its extremes, including a tile id ABOVE the old 8-bit ceiling (the
    /// whole point of the widening) and the overlay payload in its new home.
    /// If you move a field, update this decode to match the shader you edited.
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
            let packed2 = pack_vertex2(45) | pack_overlay(overlay) | pack_normal_code(5);
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
    }

    /// The greedy merge span shares the overlay payload, and `block.wgsl` reads
    /// its two nibbles at fixed shifts (`packed2 >> 20` and `>> 24`). Mirror
    /// that decode and round-trip both extremes: an off-by-one word or shift
    /// here is a stretched-texture bug nothing else catches.
    #[test]
    fn the_greedy_span_round_trips_through_the_shaders_decode() {
        for (w, h) in [(1u32, 1u32), (16, 1), (1, 16), (16, 16), (5, 11)] {
            let packed2 = pack_vertex2(9) | pack_overlay(pack_greedy_span(w, h));
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

    /// The atlas cap, the shader uv-rect table and the tile field must describe
    /// the same ceiling — the drift that would silently truncate tile ids.
    #[test]
    fn the_tile_field_sizes_the_atlas_cap_and_the_uv_rect_table() {
        assert_eq!(MAX_TILES, TILE_MASK as usize + 1);
        assert_eq!(crate::render::uniforms::UV_RECTS_LEN, MAX_TILES);
    }
}

/// Pack a linear RGB tint (each channel `0..=1` — warm/biome tints never
/// exceed 1) into the `Vertex::tint` unorm8 word, little-endian
/// `r | g<<8 | b<<16`, matching `VertexFormat::Unorm8x4`'s lane order. The
/// SINGLE owner of the tint encoding; the alpha lane is unused and fixed at
/// 255.
#[inline]
pub(crate) fn pack_tint(rgb: [f32; 3]) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    q(rgb[0]) | (q(rgb[1]) << 8) | (q(rgb[2]) << 16) | 0xFF00_0000
}

/// Inverse of [`pack_tint`] for the rare CPU path that post-processes an
/// already-built vertex (held-item warm tinting).
#[inline]
pub(crate) fn unpack_tint(tint: u32) -> [f32; 3] {
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
///   23..26 UV mode | 26 has-overlay flag | 27..32 free
///
/// Torch/block light moved to `packed2` bits 0..6 (see [`pack_vertex2`]) so the
/// shader can dim the sky term (day/night mods) without dimming torch light.
/// The overlay PAYLOAD lives in `packed2` too (see [`pack_overlay`]) — it is
/// itself a tile id, so it had to widen with the tile field and no longer fits
/// beside it.
#[inline]
pub(crate) fn pack_vertex(
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
pub(crate) const TILE_BITS: u32 = 11;
pub(crate) const TILE_MASK: u32 = (1 << TILE_BITS) - 1;
/// How many atlas tiles the vertex format can address — the ONE definition the
/// atlas loader's cap and the shader uv-rect table both derive from, so the
/// three cannot drift.
pub const MAX_TILES: usize = 1 << TILE_BITS;
pub(crate) const CORNER_SHIFT: u32 = 11;
pub(crate) const SHADE_SHIFT: u32 = 13;
pub(crate) const AO_SHIFT: u32 = 15;
pub(crate) const SKY_SHIFT: u32 = 17;

/// `Vertex::packed` bit 26. In the chunk pass it means "composite the overlay
/// payload"; the model3d pass, which never composites overlays, reuses the same
/// bit as [`SOLID_COLOR_FLAG`](crate::render::SOLID_COLOR_FLAG).
pub(crate) const OVERLAY_FLAG: u32 = 1 << 26;

/// The overlay payload's home in `packed2`, bits 20..31.
///
/// Three meanings share it, exactly as they shared the old `packed` 12..20: a
/// grass SIDE carries its overlay TILE ID (with `has_overlay` set); a greedy
/// quad carries its merged span as `(w - 1) | (h - 1) << 4`; a flowing-water TOP
/// carries its quantized flow heading. All three are read only by the pass that
/// wrote them.
pub(crate) const OVERLAY_SHIFT2: u32 = 20;
pub(crate) const OVERLAY_MASK: u32 = (1 << TILE_BITS) - 1;

/// Fold an overlay payload into `Vertex::packed2` — see [`OVERLAY_SHIFT2`].
#[inline]
pub(crate) fn pack_overlay(payload: u32) -> u32 {
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
pub(crate) fn pack_greedy_span(w: u32, h: u32) -> u32 {
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
pub(crate) fn unpack_greedy_span(packed2: u32) -> (u32, u32) {
    let payload = (packed2 >> OVERLAY_SHIFT2) & OVERLAY_MASK;
    ((payload & 0xF) + 1, ((payload >> 4) & 0xF) + 1)
}

/// Fold the second-word attributes into `Vertex::packed2` — the SINGLE owner of
/// that word's bit layout together with [`pack_cell_uv`] (mirrored by hand in
/// `block.wgsl` and `model3d.wgsl`):
///
///   0..6 block light (torches/furnaces, 0 dark..63 full)
///   | 6..16 cell-local uv ([`pack_cell_uv`], read only in [`UV_MODE_CELL_LOCAL`])
///   | 16..19 face-normal code ([`pack_normal_code`])
///   | 19 dyed flag ([`DYED_FLAG2`])
///   | 20..31 overlay payload ([`pack_overlay`]) | 31 RESERVED (zero)
///
/// The block channel is 6 bits like the sky channel so the shader's `block_term`
/// mirrors the sky curve exactly; the one remaining bit is reserved for future
/// per-vertex data and MUST stay zero until a new owner is documented here.
#[inline]
pub(crate) fn pack_vertex2(block_light: u32) -> u32 {
    block_light & 0x3F
}

/// `Vertex::packed2` bit 19: the vertex samples its tile's DYE-BASE twin
/// (desaturated, brightness-normalized — see `atlas`) instead of the base
/// tile. Set by every emitter whose face carries a `petramond:tint` multiply,
/// so the tint lands on a peak-white base and can both dye and whiten.
/// `block.wgsl` resolves it as `layer + tile count` on the terrain texture
/// array; `model3d.wgsl` as `v + 0.5` on the composed 2D atlas.
pub(crate) const DYED_FLAG2: u32 = 1 << 19;

/// Face-normal code, packed into `Vertex::packed2` bits 16..19: 0 = neutral (no
/// world-space face direction — the shader keeps the classic `SHADES` shading),
/// 1..=6 = [`super::face::Face::normal_code`] for sun-directional N·L shading in
/// `block.wgsl`.
#[inline]
pub(crate) fn pack_normal_code(code: u32) -> u32 {
    (code & 0x7) << 16
}

/// Explicit tile-local UV in 1/16ths (0..=16), packed into `Vertex::packed2`
/// bits 6..11 (u) and 11..16 (v). Shaders read it only when the vertex's UV mode
/// is [`UV_MODE_CELL_LOCAL`]; partial faces (stairs) use it to sample the
/// sub-rectangle of their tile matching the quad's position inside the cell, so
/// the shape textures as a full block with a chunk cut out.
#[inline]
pub(crate) fn pack_cell_uv(u16ths: u32, v16ths: u32) -> u32 {
    debug_assert!(u16ths <= 16 && v16ths <= 16);
    (u16ths << 6) | (v16ths << 11)
}

/// Packed UV mode field, shared by `block.wgsl` and dynamic block geometry.
pub(crate) const UV_MODE_SHIFT: u32 = 23;
pub(crate) const UV_MODE_NONE: u32 = 0;
pub(crate) const UV_MODE_THIN_U: u32 = 1;
pub(crate) const UV_MODE_THIN_V: u32 = 2;
/// The vertex carries an explicit tile-local UV in `packed2` (see [`pack_cell_uv`]).
pub(crate) const UV_MODE_CELL_LOCAL: u32 = 3;

/// GPU vertex for the chunk's bbmodel-block geometry: EXPLICIT attributes
/// (not the packed tile word), because a `.bbmodel` face carries an arbitrary
/// sub-rectangle UV into the model atlas that the tile-packed [`Vertex`] can't express.
/// `shade` is the directional face shade only and `light` carries the cell's
/// (sky, block) light fractions separately, so the world-model shader applies
/// the sim's day/night sky scale at DRAW time — a placed model darkens at night
/// exactly like the terrain around it (a remesh-time bake could not, since
/// meshes don't rebuild when the sun sets). `tint` stays the warm block-light
/// tint baked at mesh time.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
    pub tint: [f32; 3],
    /// `(sky01, block01)` light fractions (0..1 of the 6-bit channels).
    pub light: [f32; 2],
}

/// GPU vertex of the model→terrain contact-shadow stream: a non-indexed
/// triangle of the soft stamp a bbmodel block lays on the opaque terrain under
/// its bottom footprint cells and their supported one-cell dilation ring (the
/// shadow spills onto the grass next to the model). `darken` is the
/// multiplicative strength (0 = no change); the dedicated contact pass
/// multiplies the already-drawn terrain colour by `1 - darken`, fading back to
/// identity through fog. 16 bytes, deliberately minimal — the stream is sparse
/// (model cells only).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ContactShadowVertex {
    pub pos: [f32; 3],
    pub darken: f32,
}

pub struct ChunkMesh {
    pub opaque: Vec<Vertex>,
    pub opaque_idx: Vec<u32>,
    /// WATER geometry: alpha-blended, depth-READ-only (water must not occlude
    /// the terrain behind it), drawn last, farthest section first.
    pub transparent: Vec<Vertex>,
    pub transparent_idx: Vec<u32>,
    /// TRANSLUCENT-BLOCK geometry (ice): alpha-blended but depth-WRITING and
    /// drawn between opaque and water — a 3D sheet of translucent cubes needs
    /// depth to resolve its own face order (buffer order is arbitrary within
    /// a section), which water's read-only convention cannot give it.
    pub translucent: Vec<Vertex>,
    pub translucent_idx: Vec<u32>,
    /// Optional opaque LOD used for far chunks. This keeps the normal mesh
    /// byte-identical nearby while allowing far foliage to cull leaf-to-leaf
    /// internals once texture mips make the cutouts read as a dense canopy.
    pub far_opaque: Vec<Vertex>,
    pub far_opaque_idx: Vec<u32>,
    /// bbmodel-block geometry (explicit-UV [`ModelVertex`], sampling the model atlas),
    /// drawn in the renderer's dedicated model pass. Baked here at remesh like the rest
    /// of the chunk; empty for the common chunk with no bbmodel blocks.
    pub model: Vec<ModelVertex>,
    pub model_idx: Vec<u32>,
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
    pub(in crate::mesh) released: bool,
    /// `is_empty()` captured at release time, so emptiness queries stay truthful
    /// after the buffers are gone.
    pub(in crate::mesh) released_empty: bool,
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
            opaque_idx: vec![],
            transparent: vec![],
            transparent_idx: vec![],
            translucent: vec![],
            translucent_idx: vec![],
            far_opaque: vec![],
            far_opaque_idx: vec![],
            model: vec![],
            model_idx: vec![],
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
        self.opaque_idx.is_empty()
            && self.transparent_idx.is_empty()
            && self.translucent_idx.is_empty()
            && self.model_idx.is_empty()
            && self.contact.is_empty()
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    /// Free the CPU-side geometry of an uploaded mesh. `Vec::new()` (not `clear`)
    /// so the heap allocations are returned, not kept as capacity.
    pub fn release_cpu_buffers(&mut self) {
        debug_assert!(!self.mesh_dirty, "releasing a mesh that was never uploaded");
        self.released_empty = self.is_empty();
        self.released = true;
        self.opaque = Vec::new();
        self.opaque_idx = Vec::new();
        self.transparent = Vec::new();
        self.transparent_idx = Vec::new();
        self.translucent = Vec::new();
        self.translucent_idx = Vec::new();
        self.far_opaque = Vec::new();
        self.far_opaque_idx = Vec::new();
        self.model = Vec::new();
        self.model_idx = Vec::new();
        self.contact = Vec::new();
    }
}
