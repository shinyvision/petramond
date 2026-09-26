//! One section's mesh: the streams the builder emits, and the SEALED form the
//! mesh worker hands the renderer — the quad streams already in the final GPU
//! vertex format, so the render thread only copies bytes.

use super::{ContactShadowVertex, ModelVertex, TerrainVertex, Vertex};
use crate::visibility::SectionVisibility;

/// The terrain streams drawn through the shared quad index (four consecutive
/// vertices per quad, see [`super::push_back_face`]) — every [`ChunkMesh`]
/// stream but the bbmodel and contact-shadow ones. The renderer packs, patches,
/// counts and draws them by iterating [`QuadLayer::ALL`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum QuadLayer {
    /// Opaque terrain (and opaque fluids), far-LOD prefix first.
    Opaque,
    /// Translucent fluid faces, back-face culled.
    Transparent,
    /// Translucent fluid TOP faces, drawn with culling off.
    TransparentTwoSided,
    /// Translucent blocks (ice): alpha-blended but depth-writing.
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
    /// the surface stays visible from underneath. They used to be a second
    /// index winding over the same vertices; a separate cull-none draw is the
    /// index-free equivalent and rasterizes half the triangles.
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
    /// Which of the section's faces see each other through it — the
    /// renderer's occlusion-culling input. Set by the mesh worker from the
    /// section's cells; [`SectionVisibility::ALL`] (cull nothing) otherwise.
    pub visibility: SectionVisibility,
    /// The quad streams in the GPU's [`TerrainVertex`] format, by
    /// [`QuadLayer`] — filled by [`seal`](Self::seal) on the mesh worker, which
    /// frees the CPU [`Vertex`] streams in the same step.
    pub(crate) sealed_quads: [Vec<TerrainVertex>; QuadLayer::COUNT],
    pub(crate) sealed: bool,
    /// True once the CPU vertex/index buffers were released after a settled GPU
    /// upload (the geometry then lives only in the packed column buffer). A column
    /// repack cannot read a released mesh; it must force a remesh first.
    pub(crate) released: bool,
    /// `is_empty()` captured at release time, so emptiness queries stay truthful
    /// after the buffers are gone.
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

    /// A quad stream as the builder emitted it (empty once sealed).
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

    /// Convert the quad streams into the GPU's final [`TerrainVertex`] format
    /// and free the builder's [`Vertex`] streams. The mesh worker calls this on
    /// every mesh it hands over, so quantisation runs on the worker pool and
    /// the render thread's column upload is a byte copy. Idempotent.
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

    /// [`seal`](Self::seal), by value.
    pub fn into_sealed(mut self) -> Self {
        self.seal();
        self
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// A sealed quad stream, in GPU vertex format. Empty for an unsealed mesh:
    /// the renderer uploads only sealed meshes (the mesh worker seals every
    /// one), so geometry must never reach it through the builder streams.
    #[inline]
    pub fn gpu_quads(&self, layer: QuadLayer) -> &[TerrainVertex] {
        debug_assert!(
            self.sealed || self.quads(layer).is_empty(),
            "an unsealed mesh reached the GPU upload"
        );
        &self.sealed_quads[layer.index()]
    }

    /// Vertices in a quad stream, whichever form it is currently held in.
    #[inline]
    pub fn quad_len(&self, layer: QuadLayer) -> usize {
        self.quads(layer).len() + self.sealed_quads[layer.index()].len()
    }

    pub fn is_empty(&self) -> bool {
        if self.released {
            return self.released_empty;
        }
        // A chunk holding ONLY a bbmodel block (empty packed buffers) is NOT empty —
        // its geometry lives in the model stream, which must still upload + draw.
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

    /// Per-stream used bytes of the retained CPU buffers: `(opaque v, opaque i,
    /// far v, far i, transparent v, transparent i, translucent v, translucent i,
    /// model v, model i, contact v)`. For the memory census. The far lanes are
    /// always zero — the far LOD shares the opaque buffer.
    pub fn stream_bytes(&self) -> [u64; 11] {
        const M: usize = std::mem::size_of::<ModelVertex>();
        const C: usize = std::mem::size_of::<ContactShadowVertex>();
        let quad = |layer| self.quad_bytes(layer);
        [
            quad(QuadLayer::Opaque),
            0,
            // The far LOD is a prefix of the opaque stream, so it owns no
            // bytes of its own — counting them again would double-count.
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

    /// Used bytes of one quad stream in whichever form it is held.
    fn quad_bytes(&self, layer: QuadLayer) -> u64 {
        (std::mem::size_of_val(self.quads(layer))
            + self.sealed_quads[layer.index()].len() * std::mem::size_of::<TerrainVertex>())
            as u64
    }

    /// Allocated bytes of one quad stream in both forms.
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

    /// `(used bytes, allocated-capacity bytes)` of the retained CPU buffers,
    /// for the memory census.
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

    /// Free the CPU-side geometry of an uploaded mesh. `Vec::new()` (not `clear`)
    /// so the heap allocations are returned, not kept as capacity.
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
