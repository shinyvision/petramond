use super::layers::ColumnBuffer;
use super::{GeometryArena, GpuColumnMesh, Layer};
use petramond_mesh::TerrainVertex;

const STREAM_BLOCK_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) struct TerrainArenas {
    quads: GeometryArena,
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

    #[inline]
    pub fn quads(&self) -> &GeometryArena {
        &self.quads
    }

    #[inline]
    pub fn slice(&self, buffer: ColumnBuffer, layer: &Layer) -> wgpu::BufferSlice<'_> {
        self.get(buffer).slice(&layer.alloc, layer.len)
    }

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

    pub fn reserved_bytes(&self) -> u64 {
        self.quads.reserved_bytes() + self.streams.reserved_bytes()
    }

    pub fn free_bytes(&self) -> u64 {
        self.quads.free_bytes() + self.streams.free_bytes()
    }

    pub fn block_count(&self) -> usize {
        self.quads.block_count() + self.streams.block_count()
    }
}
