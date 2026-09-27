use super::{ContactShadowVertex, ModelVertex, TerrainVertex, Vertex};
use crate::visibility::SectionVisibility;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum QuadLayer {
    Opaque,
    Transparent,
    TransparentTwoSided,
    Translucent,
}

impl QuadLayer {
    pub const COUNT: usize = 4;
    pub const ALL: [QuadLayer; Self::COUNT] = [
        QuadLayer::Opaque,
        QuadLayer::Transparent,
        QuadLayer::TransparentTwoSided,
        QuadLayer::Translucent,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }
}

pub struct ChunkMesh {
    /// Opaque terrain quads, triangulation implied (see `QuadIdx`). An OPAQUE
    /// fluid's faces ride here too (its top in both windings).
    pub opaque: Vec<Vertex>,
    /// TRANSLUCENT fluid geometry: alpha-blended, depth-READ-only (a
    /// see-through body must not occlude the terrain behind it), drawn last,
    /// farthest section first. Back-face culled: an exposed side face over a
    /// shallower neighbour must not show its back as a dark sheet from inside.
    pub transparent: Vec<Vertex>,
    /// Translucent fluid TOP faces, drawn by the same pass with culling OFF so
    /// the surface stays visible from underneath. A separate cull-none draw
    /// avoids a second index winding and rasterizes half the triangles.
    pub transparent_two_sided: Vec<Vertex>,
    /// TRANSLUCENT-BLOCK geometry (ice): alpha-blended but depth-WRITING and
    /// drawn between opaque and water — a 3D sheet of translucent cubes needs
    /// depth to resolve its own face order (buffer order is arbitrary within
    /// a section), which water's read-only convention cannot give it.
    pub translucent: Vec<Vertex>,
    /// Optional opaque LOD for far chunks, expressed as a PREFIX LENGTH of
    /// [`opaque`](Self::opaque) rather than a stream of its own: the simplified
    /// canopy differs from the detailed one only by culling leaf-to-leaf
    /// internal faces, so the mesher emits those faces LAST and this records
    /// where they start. Drawing the far LOD is the same buffer with a shorter
    /// quad count — no second bake, no second upload, no duplicated VRAM.
    /// `0` means the section has no far LOD (nothing would be culled). It
    /// indexes the sealed opaque stream identically.
    pub far_opaque_len: u32,
    pub model: Vec<ModelVertex>,
    pub model_idx: Vec<u32>,
    pub model_blend_idx: Vec<u32>,
    pub contact: Vec<ContactShadowVertex>,
    pub mesh_dirty: bool,
    pub visibility: SectionVisibility,
    pub(crate) sealed_quads: [Vec<TerrainVertex>; QuadLayer::COUNT],
    pub(crate) sealed: bool,
    pub(crate) released: bool,
    pub(crate) released_empty: bool,
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
            far_opaque_len: 0,
            model: vec![],
            model_idx: vec![],
            model_blend_idx: vec![],
            contact: vec![],
            mesh_dirty: false,
            visibility: SectionVisibility::ALL,
            sealed_quads: Default::default(),
            sealed: false,
            released: false,
            released_empty: false,
        }
    }

    #[inline]
    pub fn quads(&self, layer: QuadLayer) -> &[Vertex] {
        match layer {
            QuadLayer::Opaque => &self.opaque,
            QuadLayer::Transparent => &self.transparent,
            QuadLayer::TransparentTwoSided => &self.transparent_two_sided,
            QuadLayer::Translucent => &self.translucent,
        }
    }

    fn quads_mut(&mut self, layer: QuadLayer) -> &mut Vec<Vertex> {
        match layer {
            QuadLayer::Opaque => &mut self.opaque,
            QuadLayer::Transparent => &mut self.transparent,
            QuadLayer::TransparentTwoSided => &mut self.transparent_two_sided,
            QuadLayer::Translucent => &mut self.translucent,
        }
    }

    pub fn seal(&mut self) {
        if self.sealed {
            return;
        }
        for layer in QuadLayer::ALL {
            let cpu = std::mem::take(self.quads_mut(layer));
            self.sealed_quads[layer.index()] = cpu.iter().map(TerrainVertex::from_mesh).collect();
        }
        self.sealed = true;
    }

    pub fn into_sealed(mut self) -> Self {
        self.seal();
        self
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    #[inline]
    pub fn gpu_quads(&self, layer: QuadLayer) -> &[TerrainVertex] {
        debug_assert!(
            self.sealed || self.quads(layer).is_empty(),
            "an unsealed mesh reached the GPU upload"
        );
        &self.sealed_quads[layer.index()]
    }

    #[inline]
    pub fn quad_len(&self, layer: QuadLayer) -> usize {
        self.quads(layer).len() + self.sealed_quads[layer.index()].len()
    }

    pub fn is_empty(&self) -> bool {
        if self.released {
            return self.released_empty;
        }
        QuadLayer::ALL
            .iter()
            .all(|&layer| self.quad_len(layer) == 0)
            && self.model_idx.is_empty()
            && self.model_blend_idx.is_empty()
            && self.contact.is_empty()
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    pub fn stream_bytes(&self) -> [u64; 11] {
        const M: usize = std::mem::size_of::<ModelVertex>();
        const C: usize = std::mem::size_of::<ContactShadowVertex>();
        let quad = |layer| self.quad_bytes(layer);
        [
            quad(QuadLayer::Opaque),
            0,
            0,
            0,
            quad(QuadLayer::Transparent) + quad(QuadLayer::TransparentTwoSided),
            0,
            quad(QuadLayer::Translucent),
            0,
            (self.model.len() * M) as u64,
            ((self.model_idx.len() + self.model_blend_idx.len()) * 4) as u64,
            (self.contact.len() * C) as u64,
        ]
    }

    fn quad_bytes(&self, layer: QuadLayer) -> u64 {
        (std::mem::size_of_val(self.quads(layer))
            + self.sealed_quads[layer.index()].len() * std::mem::size_of::<TerrainVertex>())
            as u64
    }

    fn quad_capacity_bytes(&self, layer: QuadLayer) -> usize {
        let cpu = match layer {
            QuadLayer::Opaque => self.opaque.capacity(),
            QuadLayer::Transparent => self.transparent.capacity(),
            QuadLayer::TransparentTwoSided => self.transparent_two_sided.capacity(),
            QuadLayer::Translucent => self.translucent.capacity(),
        };
        cpu * std::mem::size_of::<Vertex>()
            + self.sealed_quads[layer.index()].capacity() * std::mem::size_of::<TerrainVertex>()
    }

    pub fn memory_bytes(&self) -> (u64, u64) {
        const M: usize = std::mem::size_of::<ModelVertex>();
        const C: usize = std::mem::size_of::<ContactShadowVertex>();
        let quads_used: usize = QuadLayer::ALL
            .iter()
            .map(|&layer| self.quad_bytes(layer) as usize)
            .sum();
        let quads_cap: usize = QuadLayer::ALL
            .iter()
            .map(|&layer| self.quad_capacity_bytes(layer))
            .sum();
        let used = quads_used
            + self.model.len() * M
            + self.contact.len() * C
            + self.model_idx.len() * 4
            + self.model_blend_idx.len() * 4;
        let cap = quads_cap
            + self.model.capacity() * M
            + self.contact.capacity() * C
            + self.model_idx.capacity() * 4
            + self.model_blend_idx.capacity() * 4;
        (used as u64, (cap + std::mem::size_of::<Self>()) as u64)
    }

    pub fn release_cpu_buffers(&mut self) {
        debug_assert!(!self.mesh_dirty, "releasing a mesh that was never uploaded");
        self.released_empty = self.is_empty();
        self.released = true;
        for layer in QuadLayer::ALL {
            *self.quads_mut(layer) = Vec::new();
            self.sealed_quads[layer.index()] = Vec::new();
        }
        self.far_opaque_len = 0;
        self.model = Vec::new();
        self.model_idx = Vec::new();
        self.model_blend_idx = Vec::new();
        self.contact = Vec::new();
    }
}

#[cfg(test)]
mod tests;
