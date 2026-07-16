/// Per-face directional shade factors, mirrored in `block.wgsl`.
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

/// GPU vertex: 24 bytes. `pos` stays full `f32` (it keeps the water surface Y
/// baked on the CPU, and dynamic bakes — item entities, chests, doors — write
/// absolute world positions). `tint` is LINEAR RGB packed unorm8 ([`pack_tint`];
/// the GPU reads it as `Unorm8x4` — linear values in a linear-interpreted
/// format, so no sRGB OETF level shift; 1/255 steps on a multiplier that feeds
/// an 8-bit output). `packed` folds the uv tile + corner + shade index + AO
/// level into one word; the vertex shader reconstructs uv (by SELECTING from a
/// CPU-uploaded `tile_uv()` table -- never recomputing) and light (from the
/// `SHADES` literal times an AO lookup). The uv/shade decode is bit-identical to
/// the old inline values; `light` additionally folds in the per-vertex AO term.
///
/// Remaining pack lever, NOT done: quantizing `pos` to section-local fixed
/// point would reach ~16 bytes, but needs a per-column origin fed to the packed
/// column draws (instance-step buffer or per-draw uniform) AND a split format
/// for the dynamic bakes that share this layout with absolute positions.
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
/// Bit layout (mirrored by hand in `src/shaders/block.wgsl` and `model3d.wgsl`):
///   0..8 tile id | 8..10 corner (0..3) | 10..12 shade index (into `SHADES`)
///   12..20 overlay tile | 20 has-overlay flag | 21..23 AO (0 dark..3 bright)
///   23..29 SKYLIGHT ONLY (0 dark..63 full sky) | 29..32 UV mode
///
/// Torch/block light moved to `packed2` bits 0..6 (see [`pack_vertex2`]) so the
/// shader can dim the sky term (day/night mods) without dimming torch light.
///
/// `overlay`/`has_overlay` are the raw 12..20 payload and the bit-20 flag: a grass
/// SIDE sets them to `(GrassSideOverlay, true)`; a flowing-water TOP reuses the
/// same 8 bits to carry its quantized flow heading with `has_overlay = false` (so
/// the fragment shader composites no overlay); everything else passes `(0, false)`.
#[inline]
pub(crate) fn pack_vertex(
    tile: u32,
    corner: u32,
    shade_idx: u32,
    overlay: u32,
    has_overlay: bool,
    ao: u32,
    light: u32,
) -> u32 {
    tile | (corner << 8)
        | (shade_idx << 10)
        | (overlay << 12)
        | ((has_overlay as u32) << 20)
        | (ao << 21)
        | (light << 23)
}

/// Fold the second-word attributes into `Vertex::packed2` — the SINGLE owner of
/// that word's bit layout together with [`pack_cell_uv`] (mirrored by hand in
/// `block.wgsl` and `model3d.wgsl`):
///
///   0..6 block light (torches/furnaces, 0 dark..63 full)
///   | 6..16 cell-local uv ([`pack_cell_uv`], read only in [`UV_MODE_CELL_LOCAL`])
///   | 16..19 face-normal code ([`pack_normal_code`])
///   | 19..32 RESERVED (zero)
///
/// The block channel is 6 bits like the sky channel so the shader's `block_term`
/// mirrors the sky curve exactly; the remaining 13 bits are reserved for future
/// per-vertex data and MUST stay zero until a new owner is documented here.
#[inline]
pub(crate) fn pack_vertex2(block_light: u32) -> u32 {
    block_light & 0x3F
}

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
pub(crate) const UV_MODE_SHIFT: u32 = 29;
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

pub struct ChunkMesh {
    pub opaque: Vec<Vertex>,
    pub opaque_idx: Vec<u32>,
    pub transparent: Vec<Vertex>,
    pub transparent_idx: Vec<u32>,
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
            far_opaque: vec![],
            far_opaque_idx: vec![],
            model: vec![],
            model_idx: vec![],
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
        self.opaque_idx.is_empty() && self.transparent_idx.is_empty() && self.model_idx.is_empty()
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
        self.far_opaque = Vec::new();
        self.far_opaque_idx = Vec::new();
        self.model = Vec::new();
        self.model_idx = Vec::new();
    }
}
