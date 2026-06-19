/// Per-face directional shade factors, indexed by `Face::shade_idx`. The vertex
/// shader (`block.wgsl`) holds a byte-identical copy; `tests::shade_table_*`
/// locks the two in sync. Top brightest, bottom darkest.
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
    /// bits 0..8 = tile id (`Tile as u32`), 8..10 = corner (0..3),
    /// 10..12 = shade index (into `SHADES`), 12..20 = overlay tile,
    /// 20 = has-overlay flag, 21..23 = AO level (0 dark .. 3 bright),
    /// 23..29 = skylight level (0 dark .. 63 full sky).
    pub packed: u32,
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
            mesh_dirty: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.opaque_idx.is_empty() && self.transparent_idx.is_empty()
    }
}
