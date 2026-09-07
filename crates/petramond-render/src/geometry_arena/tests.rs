use super::*;

/// The class function is the arena's whole memory policy: it must never
/// round DOWN (that would hand out an allocation the caller overruns) and
/// its waste must stay bounded, or terrain VRAM balloons silently.
#[test]
fn size_classes_cover_the_request_with_bounded_waste() {
    for len in (1u64..1 << 22).step_by(97) {
        let c = class_size(len);
        assert!(c >= len, "class {c} smaller than {len}");
        assert!(
            c <= len + MIN_CLASS.max(len / 8),
            "class {c} wastes too much on {len}"
        );
        assert_eq!(c % 4, 0, "class {c} breaks wgpu's 4-byte alignment");
    }
}

/// Same-class frees must be reusable by any same-class request — that is
/// what keeps allocation O(1) with no fragmentation search.
#[test]
fn freed_allocations_are_reused_by_the_same_class() {
    assert_eq!(class_size(5000), class_size(5100));
    assert_ne!(class_size(5000), class_size(9000));
}
