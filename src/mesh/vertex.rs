/// Per-face directional shade factors, mirrored in `block.wgsl`.
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

/// GPU vertex: 32 bytes. `pos` and `tint` stay full `f32` (pos keeps the water
/// surface Y baked on the CPU; tint must not be quantized -- the sRGB OETF would
/// shift output levels). `packed` folds the uv tile + corner + shade index + AO
/// level into one word; the vertex shader reconstructs uv (by SELECTING from a
/// CPU-uploaded `tile_uv()` table -- never recomputing) and light (from the
/// `SHADES` literal times an AO lookup). The uv/shade decode is bit-identical to
/// the old inline values; `light` additionally folds in the per-vertex AO term.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub tint: [f32; 3],
    /// Folded tile + corner + shade + overlay + AO + SKY light. [`pack_vertex`] is
    /// the sole owner of this bit layout (see its doc); the vertex shader decodes
    /// it (selecting uv from the CPU-uploaded `tile_uv()` table — never recomputing
    /// — and light from `SHADES * AO`).
    pub packed: u32,
    /// Second packed word, carrying the light channels the first word has no room
    /// for. [`pack_vertex2`] is the sole owner of its bit layout.
    pub packed2: u32,
}

/// Fold one vertex's attributes into the packed `u32` word — the SINGLE owner of
/// the `Vertex::packed` bit layout. Everything that emits a mesh vertex (the chunk
/// mesher's cube faces and cross-plants; `render::block_model` mirrors the same
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
/// that word's bit layout (mirrored by hand in `block.wgsl` and `model3d.wgsl`):
///
///   0..6 block light (torches/furnaces, 0 dark..63 full) | 6..32 RESERVED (zero)
///
/// The block channel is 6 bits like the sky channel so the shader's `block_term`
/// mirrors the sky curve exactly; the remaining 26 bits are reserved for future
/// per-vertex data and MUST stay zero until a new owner is documented here.
#[inline]
pub(crate) fn pack_vertex2(block_light: u32) -> u32 {
    block_light & 0x3F
}

/// Packed UV mode field, shared by `block.wgsl` and dynamic block geometry.
pub(crate) const UV_MODE_SHIFT: u32 = 29;
pub(crate) const UV_MODE_NONE: u32 = 0;
pub(crate) const UV_MODE_THIN_U: u32 = 1;
pub(crate) const UV_MODE_THIN_V: u32 = 2;
pub(crate) const UV_MODE_STAIR_POS_X: u32 = 3;
pub(crate) const UV_MODE_STAIR_NEG_X: u32 = 4;
pub(crate) const UV_MODE_STAIR_POS_Z: u32 = 5;
pub(crate) const UV_MODE_STAIR_NEG_Z: u32 = 6;
pub(crate) const UV_MODE_STAIR_TOP: u32 = 7;

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
    /// True until GPU upload has happened. Set by `build_mesh`, cleared by
    /// renderer after a successful upload so we don't re-upload every frame.
    pub mesh_dirty: bool,
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
        }
    }

    pub fn is_empty(&self) -> bool {
        // A chunk holding ONLY a bbmodel block (empty packed buffers) is NOT empty —
        // its geometry lives in the model stream, which must still upload + draw.
        self.opaque_idx.is_empty() && self.transparent_idx.is_empty() && self.model_idx.is_empty()
    }
}
