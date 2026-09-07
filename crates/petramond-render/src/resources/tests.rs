use super::*;

/// The vertex-only patch path must refuse a mesh whose model index TOPOLOGY
/// changed even when every layer count matches — count equality alone let
/// stale GPU indices rewire a rebaked model's triangles.
#[test]
fn index_hash_distinguishes_equal_count_topologies() {
    let mut a = petramond_mesh::ChunkMesh::empty();
    let mut b = petramond_mesh::ChunkMesh::empty();
    a.model_idx = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
    b.model_idx = vec![4, 5, 6, 4, 6, 7, 0, 1, 2, 0, 2, 3];
    assert_ne!(section_index_hash(&a), section_index_hash(&b));
    assert_eq!(section_index_hash(&a), section_index_hash(&a));
}
