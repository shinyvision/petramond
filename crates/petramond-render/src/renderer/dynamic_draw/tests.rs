use super::*;

#[test]
fn a_pattern_repeats_at_the_vertex_stride() {
    let quad = prim_index_list(&[0, 1, 2, 0, 2, 3], 4, 3);
    assert_eq!(quad.len(), 18);
    assert_eq!(&quad[..6], &[0, 1, 2, 0, 2, 3]);
    assert_eq!(&quad[6..12], &[4, 5, 6, 4, 6, 7]);
    assert_eq!(&quad[12..], &[8, 9, 10, 8, 10, 11]);
    assert!(prim_index_list(&[0, 1, 2], 3, 0).is_empty());
}

#[test]
fn a_grown_vertex_buffer_is_indexed_to_its_capacity() {
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
    assert_eq!(prims_to_index(INITIAL_BYTES, prim_bytes, 100), 100);
}
