/// Per-face directional shade factors, mirrored in `block.wgsl`.
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

/// GPU vertex: 28 bytes. `pos` and `tint` stay full `f32` (pos keeps the water
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
    /// Folded tile + corner + shade + overlay + AO + skylight. [`pack_vertex`] is
    /// the sole owner of this bit layout (see its doc); the vertex shader decodes
    /// it (selecting uv from the CPU-uploaded `tile_uv()` table — never recomputing
    /// — and light from `SHADES * AO`).
    pub packed: u32,
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
///   23..29 skylight (0 dark..63 full sky)
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

/// GPU vertex for the chunk's bbmodel-block geometry: 36 bytes of EXPLICIT attributes
/// (not the packed tile word), because a `.bbmodel` face carries an arbitrary
/// sub-rectangle UV into the model atlas that the tile-packed [`Vertex`] can't express.
/// Baked at mesh time with full mesh-time lighting folded into `shade` + the warm `tint`
/// (the model pass shader does `tex * shade * tint`, like the mob pipeline whose
/// `ItemVertex` layout this mirrors byte-for-byte so they share a pipeline).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
    pub tint: [f32; 3],
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
