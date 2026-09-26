use super::book::{Book, Placement};
use super::*;

/// The class function is the arena's whole memory policy: it must never
/// round DOWN (that would hand out an allocation the caller overruns) and
/// its waste must stay bounded, or terrain VRAM balloons silently.
#[test]
fn size_classes_cover_the_request_with_bounded_waste() {
    for unit in [4u64, 16, 20, 32] {
        for len in (1u64..1 << 22).step_by(97) {
            let c = class_size(len, unit);
            assert!(c >= len, "class {c} smaller than {len} (unit {unit})");
            assert!(
                c <= len + (MIN_CLASS + unit).max(len / 8 + unit),
                "class {c} wastes too much on {len} (unit {unit})"
            );
            assert_eq!(c % unit, 0, "class {c} is not whole {unit}-byte units");
            assert_eq!(c % 4, 0, "class {c} breaks wgpu's 4-byte alignment");
        }
    }
}

/// Same-class frees must be reusable by any same-class request — that is
/// what keeps allocation O(1) with no fragmentation search.
#[test]
fn freed_allocations_are_reused_by_the_same_class() {
    assert_eq!(class_size(5000, 4), class_size(5100, 4));
    assert_ne!(class_size(5000, 4), class_size(9000, 4));
    assert_eq!(class_size(5000, 20), class_size(5100, 20));
    let mut book = Book::new(4, BLOCK_BYTES);
    let first = book.place(5000);
    let neighbour = book.place(9000);
    assert_eq!(first.new_block, Some(BLOCK_BYTES));
    assert_eq!(neighbour.new_block, None, "bumped into the same block");
    book.reclaim(vec![(first.capacity, first.block, first.offset)]);
    let reused = book.place(5100);
    assert_eq!(
        (reused.block, reused.offset, reused.new_block),
        (first.block, first.offset, None),
        "a same-class request takes the freed slot"
    );
    let other_class = book.place(9000);
    assert_ne!(other_class.offset, first.offset, "a different class bumps");
}

#[test]
fn allocations_bump_through_a_block_then_open_the_next() {
    let mut book = Book::new(4, BLOCK_BYTES);
    let big = BLOCK_BYTES / 4;
    let placed: Vec<Placement> = (0..5).map(|_| book.place(big)).collect();
    for (i, p) in placed.iter().take(4).enumerate() {
        assert_eq!((p.block, p.offset), (0, i as u64 * big));
    }
    assert_eq!((placed[4].block, placed[4].offset), (1, 0));
    assert_eq!(placed[4].new_block, Some(BLOCK_BYTES));
    assert_eq!(book.block_count(), 2);
    assert_eq!(book.reserved_bytes(), 2 * BLOCK_BYTES);
}

#[test]
fn an_oversized_request_gets_a_block_of_its_own() {
    let mut book = Book::new(4, BLOCK_BYTES);
    let huge = BLOCK_BYTES * 3 + 1;
    let p = book.place(huge);
    assert_eq!(p.offset, 0);
    assert_eq!(p.new_block, Some(class_size(huge, 4)));
    assert!(p.capacity >= huge);
}

/// Emptied blocks are released — all but one spare, kept so the streaming
/// frontier does not reallocate on every wobble — and the free entries into
/// a released block are purged, so nothing hands out memory that is gone.
#[test]
fn emptied_blocks_are_released_past_one_spare() {
    let mut book = Book::new(4, BLOCK_BYTES);
    let big = BLOCK_BYTES / 2;
    let placed: Vec<Placement> = (0..6).map(|_| book.place(big)).collect();
    assert_eq!(book.block_count(), 3);
    let freed = |p: &Placement| (p.capacity, p.block, p.offset);
    // Empty blocks 0 and 1 entirely; block 2 stays live.
    let released = book.reclaim(placed[..4].iter().map(freed).collect());
    assert_eq!(released, [1], "block 0 stays as the spare");
    assert_eq!(book.block_count(), 2);
    assert_eq!(
        book.free_bytes(),
        2 * big,
        "only the spare's entries remain free"
    );
    // The spare's space is reused before any new block opens.
    let again = book.place(big);
    assert_eq!((again.block, again.new_block), (0, None));
    // With the spare now occupied, the next new block reuses the hole.
    book.place(big);
    let fresh = book.place(big);
    assert_eq!((fresh.block, fresh.new_block), (1, Some(BLOCK_BYTES)));
}

#[test]
fn reclaiming_without_emptying_a_block_releases_nothing() {
    let mut book = Book::new(4, BLOCK_BYTES);
    let a = book.place(1000);
    book.place(1000);
    assert!(book
        .reclaim(vec![(a.capacity, a.block, a.offset)])
        .is_empty());
    assert_eq!(book.block_count(), 1);
    assert_eq!(book.free_bytes(), a.capacity);
}

/// Against a model of every live allocation over a long random mix of
/// allocations and frees: live allocations never overlap, always fit their
/// block and their request, and the free list only ever holds space no live
/// allocation uses.
#[test]
fn a_random_workload_never_overlaps_live_allocations() {
    let mut book = Book::new(4, BLOCK_BYTES);
    let mut live: Vec<Placement> = Vec::new();
    let mut block_sizes: Vec<u64> = Vec::new();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for step in 0..4000 {
        if live.is_empty() || next() % 3 != 0 {
            let len = match next() % 4 {
                0 => next() % 4096 + 1,
                1 => next() % 65_536 + 1,
                2 => next() % (1 << 20) + 1,
                _ => next() % (BLOCK_BYTES / 3) + 1,
            };
            let p = book.place(len);
            assert!(p.capacity >= len);
            if let Some(size) = p.new_block {
                let slot = p.block as usize;
                if block_sizes.len() <= slot {
                    block_sizes.resize(slot + 1, 0);
                }
                block_sizes[slot] = size;
            }
            assert!(
                p.offset + p.capacity <= block_sizes[p.block as usize],
                "step {step}: allocation leaves its block"
            );
            for other in &live {
                let disjoint = other.block != p.block
                    || p.offset + p.capacity <= other.offset
                    || other.offset + other.capacity <= p.offset;
                assert!(disjoint, "step {step}: {p:?} overlaps {other:?}");
            }
            live.push(p);
        } else {
            let count = (next() % 4 + 1) as usize;
            let mut freed = Vec::new();
            for _ in 0..count.min(live.len()) {
                let p = live.swap_remove(next() as usize % live.len());
                freed.push((p.capacity, p.block, p.offset));
            }
            book.reclaim(freed);
        }
    }
    let live_bytes: u64 = live.iter().map(|p| p.capacity).sum();
    assert!(live_bytes + book.free_bytes() <= book.reserved_bytes());
}

/// A four-byte-unit arena keeps the byte classes the per-buffer policy was
/// tuned with.
#[test]
fn the_byte_arena_keeps_its_classes() {
    assert_eq!(class_size(1, 4), 512);
    assert_eq!(class_size(4096, 4), 4096);
    assert_eq!(class_size(4100, 4), 4608);
    assert_eq!(class_size(1 << 20, 4), 1 << 20);
}

/// A vertex-stride arena only ever places allocations at whole-element
/// offsets, so a draw can reach any of them by `base_vertex`.
#[test]
fn a_vertex_arena_places_only_whole_elements() {
    let mut book = Book::new(20, BLOCK_BYTES);
    for len in [1u64, 19, 21, 999, 5000, 70_000] {
        let p = book.place(len);
        assert_eq!(p.offset % 20, 0, "{p:?}");
        assert_eq!(p.capacity % 20, 0, "{p:?}");
    }
}
