use petramond_mesh::{ChunkMesh, ContactShadowVertex, ModelVertex, QuadLayer, TerrainVertex};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ColumnBuffer {
    Quads(QuadLayer),
    ModelVertices,
    ModelIndices,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionStream {
    OpaqueFar,
    OpaqueTail,
    Transparent,
    TransparentTwoSided,
    Translucent,
    ModelVertices,
    ModelIndices,
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

    #[inline]
    pub const fn is_index(self) -> bool {
        matches!(
            self,
            SectionStream::ModelIndices | SectionStream::ModelBlendIndices
        )
    }

    #[inline]
    pub const fn quad_layer(self) -> Option<QuadLayer> {
        match self.buffer() {
            ColumnBuffer::Quads(layer) => Some(layer),
            _ => None,
        }
    }
}

pub(crate) fn far_len(mesh: &ChunkMesh) -> usize {
    let opaque = mesh.quad_len(QuadLayer::Opaque);
    if mesh.far_opaque_len > 0 {
        mesh.far_opaque_len as usize
    } else {
        opaque
    }
}

pub(crate) fn mesh_bytes(mesh: &ChunkMesh, stream: SectionStream) -> &[u8] {
    let far = far_len(mesh);
    match stream {
        SectionStream::OpaqueFar => bytemuck::cast_slice(&mesh.gpu_quads(QuadLayer::Opaque)[..far]),
        SectionStream::OpaqueTail => {
            bytemuck::cast_slice(&mesh.gpu_quads(QuadLayer::Opaque)[far..])
        }
        SectionStream::Transparent
        | SectionStream::TransparentTwoSided
        | SectionStream::Translucent => {
            bytemuck::cast_slice(mesh.gpu_quads(stream.quad_layer().expect("a quad stream")))
        }
        SectionStream::ModelVertices => bytemuck::cast_slice(&mesh.model),
        SectionStream::Contact => bytemuck::cast_slice(&mesh.contact),
        SectionStream::ModelIndices | SectionStream::ModelBlendIndices => {
            bytemuck::cast_slice(mesh_indices(mesh, stream))
        }
    }
}

pub(crate) fn mesh_indices(mesh: &ChunkMesh, stream: SectionStream) -> &[u32] {
    match stream {
        SectionStream::ModelBlendIndices => &mesh.model_blend_idx,
        _ => &mesh.model_idx,
    }
}

pub(crate) fn mesh_count(mesh: &ChunkMesh, stream: SectionStream) -> u32 {
    let elements = mesh_bytes(mesh, stream).len() as u64 / stream.buffer().stride();
    elements as u32
}

#[cfg(test)]
mod tests;
