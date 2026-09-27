use super::*;
use petramond_mesh::Vertex;

#[test]
fn every_table_indexes_its_own_slots() {
    for (i, buffer) in ColumnBuffer::ALL.into_iter().enumerate() {
        assert_eq!(buffer.index(), i, "{buffer:?}");
    }
    for (i, stream) in SectionStream::ALL.into_iter().enumerate() {
        assert_eq!(stream.index(), i, "{stream:?}");
    }
}

#[test]
fn every_buffer_is_fed_by_a_stream() {
    for buffer in ColumnBuffer::ALL {
        assert!(
            SectionStream::ALL.iter().any(|s| s.buffer() == buffer),
            "{buffer:?} has no stream"
        );
    }
    for layer in QuadLayer::ALL {
        assert!(
            SectionStream::ALL
                .iter()
                .any(|s| s.quad_layer() == Some(layer)),
            "{layer:?} has no stream"
        );
    }
}

#[test]
fn every_stride_is_copy_aligned() {
    for buffer in ColumnBuffer::ALL {
        assert_eq!(
            buffer.stride() % wgpu::COPY_BUFFER_ALIGNMENT,
            0,
            "{buffer:?}"
        );
    }
}

fn quad(y: f32) -> [Vertex; 4] {
    std::array::from_fn(|i| Vertex {
        pos: [i as f32, y, 0.0],
        tint: 0,
        packed: i as u32,
        packed2: 0,
    })
}

#[test]
fn the_opaque_stream_splits_at_the_far_lod() {
    let mut mesh = ChunkMesh::empty();
    mesh.opaque = [quad(0.0), quad(1.0), quad(2.0)].concat();
    mesh.far_opaque_len = 8;
    mesh.translucent = quad(3.0).to_vec();
    mesh.model_idx = vec![0, 1, 2];
    mesh.seal();
    assert_eq!(mesh_count(&mesh, SectionStream::OpaqueFar), 8);
    assert_eq!(mesh_count(&mesh, SectionStream::OpaqueTail), 4);
    assert_eq!(mesh_count(&mesh, SectionStream::Translucent), 4);
    assert_eq!(mesh_count(&mesh, SectionStream::Transparent), 0);
    assert_eq!(mesh_count(&mesh, SectionStream::ModelIndices), 3);
    let far = mesh_bytes(&mesh, SectionStream::OpaqueFar);
    let tail = mesh_bytes(&mesh, SectionStream::OpaqueTail);
    assert_eq!(
        [far, tail].concat(),
        bytemuck::cast_slice::<_, u8>(mesh.gpu_quads(QuadLayer::Opaque))
    );
    mesh.far_opaque_len = 0;
    assert_eq!(mesh_count(&mesh, SectionStream::OpaqueFar), 12);
    assert_eq!(mesh_count(&mesh, SectionStream::OpaqueTail), 0);
}
