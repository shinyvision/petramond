use super::*;

fn quad(y: f32, packed: u32) -> [Vertex; 4] {
    std::array::from_fn(|i| Vertex {
        pos: [i as f32 * 0.25, y, 1.5],
        tint: 0x00FF_00FF,
        packed: packed + i as u32,
        packed2: 7,
    })
}

fn mesh() -> ChunkMesh {
    let mut mesh = ChunkMesh::empty();
    mesh.opaque = [quad(64.0, 10), quad(65.0, 20)].concat();
    mesh.far_opaque_len = 4;
    mesh.transparent = quad(66.0, 30).to_vec();
    mesh.transparent_two_sided = quad(67.0, 40).to_vec();
    mesh.translucent = quad(68.0, 50).to_vec();
    mesh
}

/// Sealing is exactly the quantisation the renderer used to run at upload:
/// each quad stream becomes its `TerrainVertex` image, in order, and the
/// builder streams are freed.
#[test]
fn sealing_converts_every_quad_stream_and_frees_the_builder_copy() {
    let before = mesh();
    let mut sealed = mesh();
    sealed.seal();
    assert!(sealed.is_sealed());
    for layer in QuadLayer::ALL {
        let want: Vec<TerrainVertex> = before
            .quads(layer)
            .iter()
            .map(TerrainVertex::from_mesh)
            .collect();
        let got = sealed.gpu_quads(layer);
        assert_eq!(got.len(), want.len(), "{layer:?}");
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(bytemuck::bytes_of(g), bytemuck::bytes_of(w), "{layer:?}");
        }
        assert!(
            sealed.quads(layer).is_empty(),
            "{layer:?} builder copy kept"
        );
        assert_eq!(sealed.quad_len(layer), before.quad_len(layer));
    }
    // The far LOD prefix indexes the sealed stream identically.
    assert_eq!(sealed.far_opaque_len, 4);
    assert!(!sealed.is_empty());
}

#[test]
fn sealing_twice_changes_nothing() {
    let mut once = mesh();
    once.seal();
    let mut twice = mesh().into_sealed();
    twice.seal();
    for layer in QuadLayer::ALL {
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(once.gpu_quads(layer)),
            bytemuck::cast_slice::<_, u8>(twice.gpu_quads(layer))
        );
    }
}

/// The census and the release path see sealed bytes too.
#[test]
fn sealed_streams_are_counted_and_released() {
    let mut sealed = mesh().into_sealed();
    let t = std::mem::size_of::<TerrainVertex>() as u64;
    assert_eq!(sealed.stream_bytes()[0], 8 * t);
    assert_eq!(sealed.stream_bytes()[4], 8 * t);
    assert!(sealed.memory_bytes().0 >= 20 * t);
    sealed.release_cpu_buffers();
    assert!(!sealed.is_empty(), "emptiness stays truthful after release");
    for layer in QuadLayer::ALL {
        assert!(sealed.gpu_quads(layer).is_empty());
    }
}

#[test]
fn the_quad_layers_index_their_table_slots() {
    for (i, layer) in QuadLayer::ALL.into_iter().enumerate() {
        assert_eq!(layer.index(), i);
    }
}
