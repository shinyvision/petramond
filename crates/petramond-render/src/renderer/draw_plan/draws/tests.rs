use super::*;

fn draw(base_vertex: i32) -> IndirectDraw {
    IndirectDraw {
        index_count: 6,
        instance_count: 1,
        first_index: 0,
        base_vertex,
        first_instance: 0,
    }
}

fn list(keys: &[(QuadLayer, u32)]) -> DrawList {
    let mut list = DrawList::default();
    for (i, &key) in keys.iter().enumerate() {
        list.draws.push(draw(i as i32));
        list.keys.push(key);
        list.indices += 6;
    }
    list
}

#[test]
fn a_record_matches_wgpus_indirect_layout() {
    let ours = IndirectDraw {
        index_count: 36,
        instance_count: 1,
        first_index: 12,
        base_vertex: -4,
        first_instance: 9,
    };
    let theirs = wgpu::util::DrawIndexedIndirectArgs {
        index_count: 36,
        instance_count: 1,
        first_index: 12,
        base_vertex: -4,
        first_instance: 9,
    };
    assert_eq!(bytemuck::bytes_of(&ours), theirs.as_bytes());
    assert_eq!(DRAW_BYTES, 20);
}

#[test]
fn an_order_free_list_groups_by_block_stably() {
    let o = QuadLayer::Opaque;
    let mut l = list(&[(o, 2), (o, 0), (o, 2), (o, 1), (o, 0)]);
    l.finish(false);
    let blocks: Vec<_> = l.batches.iter().map(|b| (b.block, b.count)).collect();
    assert_eq!(blocks, [(0, 2), (1, 1), (2, 2)]);
    let order: Vec<_> = l.draws.iter().map(|d| d.base_vertex).collect();
    assert_eq!(order, [1, 4, 3, 0, 2], "stable inside each block");
    assert_eq!(l.len(), 5);
    assert_eq!(l.indices(), 30);
}

#[test]
fn an_ordered_list_batches_only_consecutive_runs() {
    let (side, top) = (QuadLayer::Transparent, QuadLayer::TransparentTwoSided);
    let mut l = list(&[(side, 0), (side, 0), (top, 0), (side, 1), (side, 0)]);
    l.finish(true);
    let batches: Vec<_> = l
        .batches
        .iter()
        .map(|b| (b.layer, b.block, b.first, b.count))
        .collect();
    assert_eq!(
        batches,
        [
            (side, 0, 0, 2),
            (top, 0, 2, 1),
            (side, 1, 3, 1),
            (side, 0, 4, 1)
        ]
    );
    let order: Vec<_> = l.draws.iter().map(|d| d.base_vertex).collect();
    assert_eq!(order, [0, 1, 2, 3, 4]);
}

#[test]
fn batches_tile_the_list() {
    let o = QuadLayer::Opaque;
    for order_matters in [false, true] {
        let mut l = list(&[(o, 3), (o, 1), (o, 3), (o, 3), (o, 0), (o, 1)]);
        l.finish(order_matters);
        let mut next = 0;
        for b in &l.batches {
            assert_eq!(b.first, next);
            next += b.count;
        }
        assert_eq!(next, l.len());
        assert!(l.keys.is_empty(), "keys are build-time only");
    }
}
