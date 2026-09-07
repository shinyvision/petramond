use super::*;

/// The pattern expands per primitive at the vertex stride — the invariant
/// every patterned draw (cubes, quads) relies on to index a grown buffer.
#[test]
fn a_pattern_repeats_at_the_vertex_stride() {
    let quad = prim_index_list(&[0, 1, 2, 0, 2, 3], 4, 3);
    assert_eq!(quad.len(), 18);
    assert_eq!(&quad[..6], &[0, 1, 2, 0, 2, 3]);
    assert_eq!(&quad[6..12], &[4, 5, 6, 4, 6, 7]);
    assert_eq!(&quad[12..], &[8, 9, 10, 8, 10, 11]);
    assert!(prim_index_list(&[0, 1, 2], 3, 0).is_empty());
}

/// After a vertex buffer GROWS, the regenerated index list covers every
/// primitive the grown buffer can hold — never fewer than were baked, and
/// exactly the buffer's capacity, so a later frame that fills the
/// headroom draws fully indexed without another regeneration.
#[test]
fn a_grown_vertex_buffer_is_indexed_to_its_capacity() {
    // A 32-byte quad vertex, 4 per primitive.
    let prim_bytes = 4 * 32;
    for prims in [1u32, 33, 500, 4097] {
        let needed = prims as u64 * prim_bytes;
        let grown = grown_size(needed);
        assert!(grown >= needed, "growth holds what was baked");
        assert_eq!(grown % INITIAL_BYTES, 0, "growth is page-granular");
        let indexed = prims_to_index(grown, prim_bytes, prims);
        assert!(indexed >= prims, "every baked primitive is indexed");
        assert_eq!(
            indexed as u64,
            grown / prim_bytes,
            "the index list spans the whole grown buffer"
        );
        assert!(
            (indexed as u64 + 1) * prim_bytes > grown,
            "and not one primitive the buffer cannot hold"
        );
    }
    // A buffer that already holds the baked count is not the growth
    // case; the indexed count still never drops below the baked one.
    assert_eq!(prims_to_index(INITIAL_BYTES, prim_bytes, 100), 100);
}
