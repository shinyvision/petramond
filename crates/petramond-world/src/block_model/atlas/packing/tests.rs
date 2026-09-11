use super::*;

#[test]
fn a_long_strip_wraps_without_overlap_or_losing_entries() {
    let sizes: Vec<_> = (0..700).map(|i| [8 + i % 5 * 8, 8 + i % 7 * 8]).collect();
    let packed = pack(&sizes, 2048).unwrap();
    assert_eq!(packed.origins.len(), sizes.len());
    assert_eq!(packed.origins, pack(&sizes, 2048).unwrap().origins);
    for (i, (&[x, y], &[w, h])) in packed.origins.iter().zip(&sizes).enumerate() {
        assert!(x + w <= packed.size[0] && y + h <= packed.size[1]);
        for (j, &[c, d]) in sizes.iter().take(i).enumerate() {
            let [a, b] = packed.origins[j];
            assert!(x + w <= a || a + c <= x || y + h <= b || b + d <= y);
        }
    }
}

#[test]
fn overfull_and_oversized_inputs_fail_before_allocating_pixels() {
    assert!(pack(&[[65, 1]], 64).is_err());
    assert!(pack(&[[64, 64]; 2], 64).is_err());
    assert!(pack(&[[0, 1]], 64).is_err());
    assert_eq!(pack(&[], 64).unwrap().size, [1, 1]);
}
