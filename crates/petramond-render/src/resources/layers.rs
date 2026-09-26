//! The terrain layer table: which GPU buffers a packed column owns, which
//! ranges each section owns inside them, and where each range's bytes come
//! from. Packing, patching, the memory census, the planner and the passes all
//! iterate these tables instead of naming layers one by one, so a new layer
//! is a new row here.

use petramond_mesh::{ChunkMesh, ContactShadowVertex, ModelVertex, QuadLayer, TerrainVertex};

/// One arena buffer a packed column owns.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ColumnBuffer {
    /// A quad-index stream (see [`QuadLayer`]): vertices only, drawn through
    /// the shared quad index buffer.
    Quads(QuadLayer),
    /// The bbmodel-block vertex stream.
    ModelVertices,
    /// The bbmodel index stream: every section's opaque faces, then every
    /// section's blend faces.
    ModelIndices,
    /// The non-indexed contact-shadow stream.
    Contact,
}

impl ColumnBuffer {
    pub const COUNT: usize = QuadLayer::COUNT + 3;
    pub const ALL: [ColumnBuffer; Self::COUNT] = [
        ColumnBuffer::Quads(QuadLayer::Opaque),
        ColumnBuffer::Quads(QuadLayer::Transparent),
        ColumnBuffer::Quads(QuadLayer::TransparentTwoSided),
        ColumnBuffer::Quads(QuadLayer::Translucent),
        ColumnBuffer::ModelVertices,
        ColumnBuffer::ModelIndices,
        ColumnBuffer::Contact,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        match self {
            ColumnBuffer::Quads(layer) => layer.index(),
            ColumnBuffer::ModelVertices => QuadLayer::COUNT,
            ColumnBuffer::ModelIndices => QuadLayer::COUNT + 1,
            ColumnBuffer::Contact => QuadLayer::COUNT + 2,
        }
    }

    /// Bytes per element.
    #[inline]
    pub const fn stride(self) -> u64 {
        (match self {
            ColumnBuffer::Quads(_) => std::mem::size_of::<TerrainVertex>(),
            ColumnBuffer::ModelVertices => std::mem::size_of::<ModelVertex>(),
            ColumnBuffer::ModelIndices => std::mem::size_of::<u32>(),
            ColumnBuffer::Contact => std::mem::size_of::<ContactShadowVertex>(),
        }) as u64
    }
}

/// A contiguous element range inside a column buffer.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub count: u32,
}

impl Span {
    #[inline]
    pub fn end(self) -> u32 {
        self.start + self.count
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.count == 0
    }
}

/// One range a section owns. Streams sharing a buffer lay the buffer out as
/// consecutive REGIONS, in this table's order: every section's far opaque
/// range, then every section's leaf tail; every section's opaque model
/// indices, then every section's blend indices. So each region is one
/// contiguous range per column — a column drawn wholly at far LOD, or its
/// whole blend pass, is one draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionStream {
    /// The opaque stream MINUS its leaf-to-leaf internal faces — the section's
    /// far (simplified-canopy) LOD, and a prefix of what the mesher emitted.
    OpaqueFar,
    /// The leaf-to-leaf internal faces. Empty for nearly every section.
    OpaqueTail,
    Transparent,
    /// The cull-none fluid-top stream.
    TransparentTwoSided,
    Translucent,
    ModelVertices,
    ModelIndices,
    /// The alpha-BLEND model faces: indices into the same model vertices.
    ModelBlendIndices,
    /// Contact-shadow vertices. Kept per section only so the planner can
    /// decide column contact visibility from the VISIBLE sections (the draw
    /// is whole-column): a multi-cell model's contact triangles can sit in a
    /// section whose model index range is empty.
    Contact,
}

impl SectionStream {
    pub const COUNT: usize = 9;
    pub const ALL: [SectionStream; Self::COUNT] = [
        SectionStream::OpaqueFar,
        SectionStream::OpaqueTail,
        SectionStream::Transparent,
        SectionStream::TransparentTwoSided,
        SectionStream::Translucent,
        SectionStream::ModelVertices,
        SectionStream::ModelIndices,
        SectionStream::ModelBlendIndices,
        SectionStream::Contact,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The buffer this stream's region lives in.
    #[inline]
    pub const fn buffer(self) -> ColumnBuffer {
        match self {
            SectionStream::OpaqueFar | SectionStream::OpaqueTail => {
                ColumnBuffer::Quads(QuadLayer::Opaque)
            }
            SectionStream::Transparent => ColumnBuffer::Quads(QuadLayer::Transparent),
            SectionStream::TransparentTwoSided => {
                ColumnBuffer::Quads(QuadLayer::TransparentTwoSided)
            }
            SectionStream::Translucent => ColumnBuffer::Quads(QuadLayer::Translucent),
            SectionStream::ModelVertices => ColumnBuffer::ModelVertices,
            SectionStream::ModelIndices | SectionStream::ModelBlendIndices => {
                ColumnBuffer::ModelIndices
            }
            SectionStream::Contact => ColumnBuffer::Contact,
        }
    }

    /// Index streams hold SECTION-LOCAL indices, rebased onto the section's
    /// model vertex range when they are packed.
    #[inline]
    pub const fn is_index(self) -> bool {
        matches!(
            self,
            SectionStream::ModelIndices | SectionStream::ModelBlendIndices
        )
    }

    /// The vertex stream drawn from this quad stream's layer, if it is one.
    #[inline]
    pub const fn quad_layer(self) -> Option<QuadLayer> {
        match self.buffer() {
            ColumnBuffer::Quads(layer) => Some(layer),
            _ => None,
        }
    }
}

/// A section's far-LOD vertex count: its opaque stream without the leaf-to-leaf
/// internal faces the mesher appended last. A section with no far LOD keeps its
/// whole stream, so it lands entirely in the column's far region and draws
/// identically under either LOD.
pub(crate) fn far_len(mesh: &ChunkMesh) -> usize {
    let opaque = mesh.quad_len(QuadLayer::Opaque);
    if mesh.far_opaque_len > 0 {
        mesh.far_opaque_len as usize
    } else {
        opaque
    }
}

/// The bytes a sealed mesh contributes to a VERTEX stream (index streams go
/// through [`mesh_indices`]).
pub(crate) fn mesh_bytes(mesh: &ChunkMesh, stream: SectionStream) -> &[u8] {
    let far = far_len(mesh);
    match stream {
        SectionStream::OpaqueFar => {
            bytemuck::cast_slice(&mesh.gpu_quads(QuadLayer::Opaque)[..far])
        }
        SectionStream::OpaqueTail => {
            bytemuck::cast_slice(&mesh.gpu_quads(QuadLayer::Opaque)[far..])
        }
        SectionStream::Transparent
        | SectionStream::TransparentTwoSided
        | SectionStream::Translucent => bytemuck::cast_slice(
            mesh.gpu_quads(stream.quad_layer().expect("a quad stream")),
        ),
        SectionStream::ModelVertices => bytemuck::cast_slice(&mesh.model),
        SectionStream::Contact => bytemuck::cast_slice(&mesh.contact),
        SectionStream::ModelIndices | SectionStream::ModelBlendIndices => {
            bytemuck::cast_slice(mesh_indices(mesh, stream))
        }
    }
}

/// A mesh's section-local indices for an index stream.
pub(crate) fn mesh_indices(mesh: &ChunkMesh, stream: SectionStream) -> &[u32] {
    match stream {
        SectionStream::ModelBlendIndices => &mesh.model_blend_idx,
        _ => &mesh.model_idx,
    }
}

/// Elements a mesh contributes to `stream`.
pub(crate) fn mesh_count(mesh: &ChunkMesh, stream: SectionStream) -> u32 {
    let elements = mesh_bytes(mesh, stream).len() as u64 / stream.buffer().stride();
    elements as u32
}

#[cfg(test)]
mod tests;
