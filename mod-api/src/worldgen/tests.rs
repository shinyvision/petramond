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
}
