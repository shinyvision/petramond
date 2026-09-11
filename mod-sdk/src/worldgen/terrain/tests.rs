use super::*;

#[test]
fn negative_boundaries_and_failed_reads_do_not_poison_the_cache() {
    let mut cache = TerrainCache::new(1);
    let pos = [-1, -17, 16];
    let inside = |p: [i32; 3]| {
        let [x, y, z] = p.map(|v| v.rem_euclid(16) as usize);
        (y * 16 + z) * 16 + x
    };
    assert_eq!(cache.block_with(pos, |_| Vec::new()), None);
    assert_eq!(
        cache.block_with(pos, |section| {
            assert_eq!(section, [-1, -2, 1]);
            (0..4096)
                .map(|i| BlockId(if i == inside(pos) { 9 } else { 2 }))
                .collect()
        }),
        Some(BlockId(9))
    );
    assert_eq!(
        cache.block_with(pos, |_| panic!("cached")),
        Some(BlockId(9))
    );
    cache.block_with([0, 0, 0], |_| vec![BlockId(1); 4096]);
    assert_eq!(
        cache.block_with(pos, |_| vec![BlockId(3); 4096]),
        Some(BlockId(3))
    );
}
