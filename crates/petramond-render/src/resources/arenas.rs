//! The two arenas terrain columns allocate from.

use super::layers::ColumnBuffer;
use super::{GeometryArena, GpuColumnMesh, Layer};
use petramond_mesh::TerrainVertex;

/// Block size of the sparse-stream arena. Model and contact streams exist in
/// few columns, so a full terrain-sized block would mostly reserve nothing.
const STREAM_BLOCK_BYTES: u64 = 4 * 1024 * 1024;

/// Terrain GPU storage, split by how it is drawn.
pub(crate) struct TerrainArenas {
    /// Every quad-layer vertex, in [`TerrainVertex`] units: a pass binds one
    /// block's buffer and reaches every column in it by `base_vertex`, which
    /// is what the indirect multi-draw batches over.
    quads: GeometryArena,
    /// Model vertices, model indices and contact-shadow vertices: sparse,
    /// mixed strides, bound per column.
    streams: GeometryArena,
}

impl Default for TerrainArenas {
    fn default() -> Self {
        Self {
            quads: GeometryArena::with_unit(std::mem::size_of::<TerrainVertex>() as u64),
            streams: GeometryArena::new(wgpu::COPY_BUFFER_ALIGNMENT, STREAM_BLOCK_BYTES),
        }
    }
}

impl TerrainArenas {
    /// The arena a column buffer allocates from.
    #[inline]
    pub fn get(&self, buffer: ColumnBuffer) -> &GeometryArena {
        match buffer {
            ColumnBuffer::Quads(_) => &self.quads,
            _ => &self.streams,
        }
    }

    #[inline]
    pub fn get_mut(&mut self, buffer: ColumnBuffer) -> &mut GeometryArena {
        match buffer {
            ColumnBuffer::Quads(_) => &mut self.quads,
            _ => &mut self.streams,
        }
    }

    /// The quad-layer arena, whose blocks the terrain passes bind whole.
    #[inline]
    pub fn quads(&self) -> &GeometryArena {
        &self.quads
    }

    /// The bound range of a column buffer's live bytes.
    #[inline]
    pub fn slice(&self, buffer: ColumnBuffer, layer: &Layer) -> wgpu::BufferSlice<'_> {
        self.get(buffer).slice(&layer.alloc, layer.len)
    }

    /// Bind a column's model vertex and index streams (slot 0 and the index
    /// buffer) for an indexed model draw. False when it has none.
    pub fn bind_model_streams(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        column: &GpuColumnMesh,
    ) -> bool {
        let (Some(vertices), Some(indices)) = (
            column.buffer(ColumnBuffer::ModelVertices),
            column.buffer(ColumnBuffer::ModelIndices),
        ) else {
            return false;
        };
        pass.set_vertex_buffer(0, self.slice(ColumnBuffer::ModelVertices, vertices));
        pass.set_index_buffer(
            self.slice(ColumnBuffer::ModelIndices, indices),
            wgpu::IndexFormat::Uint32,
        );
        true
    }

    /// Total bytes of GPU buffer both arenas hold.
    pub fn reserved_bytes(&self) -> u64 {
        self.quads.reserved_bytes() + self.streams.reserved_bytes()
    }

    /// Arena bytes reserved but held by no live column.
    pub fn free_bytes(&self) -> u64 {
        self.quads.free_bytes() + self.streams.free_bytes()
    }

    pub fn block_count(&self) -> usize {
        self.quads.block_count() + self.streams.block_count()
    }
}
