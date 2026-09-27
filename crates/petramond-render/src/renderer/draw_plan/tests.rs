use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo) as u64) as i32
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.next() as usize % (i + 1));
        }
    }
}

fn section(dist_sq: f32, cx: i32, cz: i32) -> VisibleSection {
    VisibleSection {
        dist_sq,
        column_pos: ChunkPos::new(cx, cz),
        column_slot: 0,
        opaque_batched: false,
        model_batched: false,
        use_far_leaf_lod: false,
        spans: Default::default(),
    }
}

fn with(mut section: VisibleSection, stream: SectionStream, count: u32) -> VisibleSection {
    section.spans[stream.index()].count = count;
    section
}

fn columns(rng: &mut Rng, side: i32) -> Vec<ColumnCull> {
    let mut out = Vec::new();
    for cx in -side / 2..side / 2 {
        for cz in -side / 2..side / 2 {
            let (min_cy, max_cy) = if rng.next().is_multiple_of(11) {
                (i32::MAX, i32::MIN)
            } else {
                let lo = rng.range(-4, 8);
                (lo, lo + rng.range(0, 6))
            };
            out.push(ColumnCull {
                pos: ChunkPos::new(cx, cz),
                slot: out.len() as ColumnSlot,
                min_cy,
                max_cy,
            });
        }
    }
    rng.shuffle(&mut out);
    out
}

fn camera_frustum(eye: glam::Vec3, target: glam::Vec3) -> Frustum {
    let proj = glam::Mat4::perspective_rh(1.2, 16.0 / 9.0, 0.1, 2000.0);
    Frustum::from_view_proj(proj * glam::Mat4::look_at_rh(eye, target, glam::Vec3::Y))
}

#[test]
fn cull_regions_partition_the_index_into_contiguous_square_runs() {
    let mut rng = Rng(0x5EED);
    let mut index = columns(&mut rng, 40);
    let mut regions = Vec::new();
    group_cull_regions(&mut index, &mut regions);
    let mut next = 0u32;
    for region in &regions {
        assert_eq!(region.first, next, "regions are back to back");
        assert!(region.last > region.first, "no empty region");
        let run = &index[region.first as usize..region.last as usize];
        for entry in run {
            assert_eq!(
                entry.pos.cx >> CULL_REGION_SHIFT << CULL_REGION_SHIFT,
                region.cx
            );
            assert_eq!(
                entry.pos.cz >> CULL_REGION_SHIFT << CULL_REGION_SHIFT,
                region.cz
            );
        }
        assert_eq!(region.min_cy, run.iter().map(|e| e.min_cy).min().unwrap());
        assert_eq!(region.max_cy, run.iter().map(|e| e.max_cy).max().unwrap());
        next = region.last;
    }
    assert_eq!(next as usize, index.len(), "every column is in a region");
}

#[test]
fn cull_regions_do_not_depend_on_the_column_maps_order() {
    let mut rng = Rng(0xC0FFEE);
    let mut a = columns(&mut rng, 24);
    let mut b = a.clone();
    rng.shuffle(&mut b);
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    group_cull_regions(&mut a, &mut ra);
    group_cull_regions(&mut b, &mut rb);
    let key = |e: &ColumnCull| (e.pos, e.slot);
    assert_eq!(
        a.iter().map(key).collect::<Vec<_>>(),
        b.iter().map(key).collect::<Vec<_>>()
    );
    let bounds = |r: &CullRegion| (r.cx, r.cz, r.min_cy, r.max_cy, r.first, r.last);
    assert_eq!(
        ra.iter().map(bounds).collect::<Vec<_>>(),
        rb.iter().map(bounds).collect::<Vec<_>>()
    );
}

#[test]
fn the_region_test_never_changes_what_is_visible() {
    let mut rng = Rng(0xFACADE);
    let origin = glam::IVec3::new(0, 64, 0);
    for view in 0..24 {
        let mut index = columns(&mut rng, 48);
        let mut regions = Vec::new();
        group_cull_regions(&mut index, &mut regions);
        let eye = glam::Vec3::new(
            rng.range(-64, 64) as f32,
            rng.range(-32, 96) as f32,
            rng.range(-64, 64) as f32,
        );
        let target = eye
            + glam::Vec3::new(
                rng.range(-10, 11) as f32,
                rng.range(-6, 7) as f32,
                rng.range(-10, 11) as f32 + 0.5,
            );
        let frustum = camera_frustum(eye, target);
        let fog = 64.0 + (view * 16) as f32;
        for region in &regions {
            let verdict = region_visible(region, &frustum, origin, eye, fog);
            for entry in &index[region.first as usize..region.last as usize] {
                let visible = Renderer::column_visible(entry, &frustum, origin, eye, fog, false);
                if visible {
                    assert!(
                        verdict.is_some(),
                        "view {view}: region rejected a visible column"
                    );
                }
                if verdict == Some(true) {
                    assert_eq!(
                        Renderer::column_visible(entry, &frustum, origin, eye, fog, true),
                        visible,
                        "view {view}: an enclosed region changed a column's verdict"
                    );
                }
            }
        }
    }
}

fn visible(has_opaque: bool, any_far_lod: bool, all_far: bool) -> VisibleColumn {
    VisibleColumn {
        has_opaque,
        any_far_lod,
        all_far_capable_are_far: all_far,
        ..VisibleColumn::default()
    }
}

const STREAMS: ColumnStreams = ColumnStreams {
    opaque_quads: 100,
    opaque_far_quads: 60,
    model_idx_count: 30,
    contact_vertex_count: 12,
};

#[test]
fn a_column_batches_its_opaque_stream_when_its_sections_agree() {
    assert_eq!(
        batch_column(visible(true, false, true), STREAMS, false).opaque,
        Some(false)
    );
    assert_eq!(
        batch_column(visible(true, true, true), STREAMS, false).opaque,
        Some(true)
    );
    assert_eq!(
        batch_column(visible(true, true, false), STREAMS, false).opaque,
        None
    );
    assert_eq!(
        batch_column(visible(false, false, true), STREAMS, false).opaque,
        None
    );
    let empty = ColumnStreams {
        opaque_quads: 0,
        opaque_far_quads: 0,
        ..STREAMS
    };
    assert_eq!(
        batch_column(visible(true, false, true), empty, false).opaque,
        None
    );
}

#[test]
fn a_culled_opaque_section_splits_the_column_unless_draws_are_direct() {
    let column = VisibleColumn {
        hidden_opaque: true,
        ..visible(true, false, true)
    };
    assert_eq!(batch_column(column, STREAMS, false).opaque, None);
    assert_eq!(batch_column(column, STREAMS, true).opaque, Some(false));
}

#[test]
fn model_and_contact_batches_follow_their_own_presence_bits() {
    let column = VisibleColumn {
        has_model: true,
        ..VisibleColumn::default()
    };
    let batch = batch_column(column, STREAMS, false);
    assert!(batch.model && !batch.contact);
    let column = VisibleColumn {
        has_contact: true,
        ..VisibleColumn::default()
    };
    let batch = batch_column(column, STREAMS, false);
    assert!(!batch.model && batch.contact);
    let no_streams = ColumnStreams {
        model_idx_count: 0,
        contact_vertex_count: 0,
        ..STREAMS
    };
    let column = VisibleColumn {
        has_model: true,
        has_contact: true,
        ..VisibleColumn::default()
    };
    let batch = batch_column(column, no_streams, false);
    assert!(!batch.model && !batch.contact);
}

#[test]
fn sections_covered_by_whole_column_draws_are_dropped() {
    let opaque = with(section(1.0, 0, 0), SectionStream::OpaqueFar, 16);
    let mut leafy_far = with(section(2.0, 0, 0), SectionStream::OpaqueTail, 16);
    leafy_far.use_far_leaf_lod = true;
    let water = with(
        with(section(3.0, 0, 0), SectionStream::OpaqueFar, 16),
        SectionStream::Transparent,
        8,
    );
    let model = with(section(4.0, 0, 0), SectionStream::ModelIndices, 6);
    let kept_before = with(section(0.5, 9, 9), SectionStream::OpaqueFar, 4);
    let mut sections = vec![kept_before, opaque, leafy_far, water, model];

    let batch = ColumnBatch {
        opaque: Some(false),
        model: true,
        contact: false,
    };
    retain_uncovered(&mut sections, 1, batch);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].column_pos, ChunkPos::new(9, 9));
    assert!(!sections[0].opaque_batched);
    assert_eq!(sections[1].dist_sq, 3.0);
    assert!(sections[1].opaque_batched && sections[1].model_batched);

    let mut sections = vec![opaque, leafy_far, water, model];
    retain_uncovered(
        &mut sections,
        0,
        ColumnBatch {
            opaque: None,
            model: false,
            contact: false,
        },
    );
    let kept: Vec<f32> = sections.iter().map(|s| s.dist_sq).collect();
    assert_eq!(kept, [1.0, 3.0, 4.0]);
}

#[test]
fn the_section_sort_is_a_total_order_independent_of_input_order() {
    let mut rng = Rng(0xD15EA5E);
    let mut sections: Vec<VisibleSection> = (0..300)
        .map(|_| section(rng.range(0, 8) as f32, rng.range(-3, 3), rng.range(-3, 3)))
        .collect();
    let mut shuffled = sections.clone();
    rng.shuffle(&mut shuffled);
    let (mut keys, mut sorted) = (Vec::new(), Vec::new());
    sort_sections(&mut sections, &mut keys, &mut sorted);
    sort_sections(&mut shuffled, &mut keys, &mut sorted);
    let key = |s: &VisibleSection| (s.dist_sq.to_bits(), s.column_pos);
    let a: Vec<_> = sections.iter().map(key).collect();
    let b: Vec<_> = shuffled.iter().map(key).collect();
    assert_eq!(a, b, "same scene, same order");
    assert!(
        sections
            .windows(2)
            .all(|w| (w[0].dist_sq, w[0].column_pos) <= (w[1].dist_sq, w[1].column_pos)),
        "near to far, ties by column"
    );
}

#[test]
fn equal_distances_break_ties_on_the_column_then_the_build_order() {
    let first_built = with(section(5.0, 1, 0), SectionStream::OpaqueFar, 4);
    let second_built = with(section(5.0, 1, 0), SectionStream::OpaqueFar, 8);
    let mut sections = vec![
        first_built,
        section(5.0, 0, 7),
        second_built,
        section(1.0, 4, 4),
    ];
    let (mut keys, mut sorted) = (Vec::new(), Vec::new());
    sort_sections(&mut sections, &mut keys, &mut sorted);
    let order: Vec<_> = sections
        .iter()
        .map(|s| (s.column_pos, s.span(SectionStream::OpaqueFar).count))
        .collect();
    assert_eq!(
        order,
        [
            (ChunkPos::new(4, 4), 0),
            (ChunkPos::new(0, 7), 0),
            (ChunkPos::new(1, 0), 4),
            (ChunkPos::new(1, 0), 8),
        ]
    );
}

#[test]
fn whole_column_draws_sort_near_to_far() {
    let mut plan = TerrainPlan {
        opaque_columns: vec![
            (9.0, ChunkPos::new(0, 0), 0, false),
            (1.0, ChunkPos::new(2, 0), 1, true),
            (1.0, ChunkPos::new(1, 0), 2, false),
        ],
        model_columns: vec![(3.0, ChunkPos::new(0, 0), 0), (2.0, ChunkPos::new(5, 5), 1)],
        ..TerrainPlan::default()
    };
    sort_columns(&mut plan);
    let opaque: Vec<_> = plan.opaque_columns.iter().map(|c| c.2).collect();
    assert_eq!(opaque, [2, 1, 0]);
    let model: Vec<_> = plan.model_columns.iter().map(|c| c.2).collect();
    assert_eq!(model, [1, 0]);
}
