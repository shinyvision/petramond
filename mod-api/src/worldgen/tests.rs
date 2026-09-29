use super::*;

#[test]
fn feature_bounds_include_cross_section_writes_and_do_not_overflow() {
    let filter = GenFeatureFilter::surface_band(-2, 1);
    assert!(filter.intersects(-1, &[0]));
    assert!(filter.intersects(0, &[0]));
    assert!(!filter.intersects(1, &[0]));
    assert!(!filter.intersects(0, &[]));
    assert!(!filter.intersects(i32::MAX, &[i32::MAX]));
    assert!(!filter.intersects(i32::MIN, &[i32::MIN]));
    let bounded = GenFeatureFilter::y_band(-1, 0);
    assert!(bounded.intersects(-1, &[]));
    assert!(bounded.intersects(0, &[]));
    assert!(!bounded.intersects(1, &[]));
    assert!(GenFeatureFilter::ANY.intersects(1 << 20, &[]));
    assert!(GenFeatureFilter::ANY.intersects(-(1 << 20), &[]));
}

#[test]
fn inverted_bands_are_invalid_and_constructors_compose() {
    assert!(!GenFeatureFilter::y_band(1, 0).is_valid());
    assert!(!GenFeatureFilter::surface_band(1, 0).is_valid());
    assert!(GenFeatureFilter::y_band(0, 0).is_valid());
    let positional = GenFeatureFilter::y_band(-64, -40).without_blocks();
    assert!(!positional.needs_blocks);
    assert_eq!(positional.min_y, -64);
    assert!(!GenFeatureFilter::ANY
        .near_underground_biomes([1], -1, 0, 0)
        .is_valid());
}

#[test]
fn biome_sets_and_gates_cover_their_edges() {
    let set = BiomeSet::new([0, 63, 64, 255]);
    assert!(set.contains(0) && set.contains(63) && set.contains(64) && set.contains(255));
    assert!(!set.contains(1) && !set.contains(128));
    assert!(set.intersects(&BiomeSet::new([255]).0));
    assert!(!set.intersects(&BiomeSet::new([2, 130]).0));
    assert!(BiomeSet::new([]).is_empty());
    let filter = GenFeatureFilter::ANY.in_surface_biomes([7]);
    assert!(filter.admits_columns(&[1, 7]));
    assert!(!filter.admits_columns(&[1, 8]));
    assert!(GenFeatureFilter::ANY.admits_columns(&[]));
    let gate = UndergroundGate {
        biomes: BiomeSet::new([3]),
        xz: 2,
        down: 30,
        up: 5,
    };
    assert_eq!(gate.around([1, -1, -2]), ([14, -46, -34], [33, 4, -15]));
    let far = gate.around([i32::MAX, i32::MIN, 0]);
    assert!(far.0[0] < far.1[0] && far.0[1] < far.1[1]);
}

#[test]
fn leaf_masks_index_leaves_x_fastest_and_admit_outside() {
    let mut mask = LeafMask::over(8, [-3, 0, 5], [20, 17, 18]);
    assert_eq!((mask.min, mask.size), ([-1, 0, 0], [4, 3, 3]));
    assert!(mask.is_well_formed() && !mask.any());
    let i = mask.index([9, 8, 16]).unwrap();
    assert_eq!(i, 22, "row 1 of 3, column 2 of 4, x 2: x fastest");
    mask.set(i);
    assert!(mask.may_hold([15, 15, 23]) && !mask.may_hold([7, 15, 23]));
    assert!(mask.column_may_hold(9, 16) && !mask.column_may_hold(0, 16));
    let column = mask.column(9, 16);
    assert!(column.any() && column.may_hold(8) && !column.may_hold(0) && !column.may_hold(16));
    assert!(column.may_hold(-100), "below the box is unknown");
    assert!(
        mask.column(100, 100).may_hold(0),
        "outside the box is unknown"
    );
    assert!(!mask.complete());
    assert!(mask.may_hold([100, 100, 100]), "outside the box is unknown");
    assert!(mask.any());
    let all = LeafMask::everywhere();
    assert!(all.may_hold([1, 2, 3]) && all.any() && all.is_well_formed());
}
