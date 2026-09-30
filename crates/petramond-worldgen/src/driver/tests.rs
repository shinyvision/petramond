use std::sync::atomic::{AtomicUsize, Ordering};

use petramond_world::block::Block;

use super::*;

#[test]
fn slimmed_column_regenerates_identical_sections() {
    let generator = ChunkGenerator::new(0xDEAD_BEEF);
    for (cx, cz) in [(0, 0), (3, -2)] {
        let full = generator.generate_column_gen(cx, cz);
        let slim = full.slimmed();
        assert!(full.has_feature_windows() && !slim.has_feature_windows());
        let (lo, hi) = full.surf_range();
        for cy in [
            lo.div_euclid(16) - 1,
            hi.div_euclid(16),
            hi.div_euclid(16) + 1,
        ] {
            let sp = SectionPos::new(cx, cy, cz);
            let a = generator.generate_section(sp, &full);
            let b = generator.generate_section(sp, &slim);
            assert_eq!(
                a.blocks_iter().collect::<Vec<_>>(),
                b.blocks_iter().collect::<Vec<_>>(),
                "slimmed rebuild diverged at cy {cy} of column ({cx},{cz})"
            );
        }
    }
}

#[test]
fn cache_record_roundtrip_matches_the_live_column() {
    let seed = 0xDEAD_BEEF;
    let generator = ChunkGenerator::new(seed);
    let full = generator.generate_column_gen(2, -5);
    let blob = crate::colgen::encode_record(&full.cache_record(seed));
    let rec =
        crate::colgen::decode_record(petramond_world::chunk::ChunkPos::new(2, -5), seed, &blob)
            .expect("cache record decodes");
    let cached = ColumnGen::from_cache_record(rec);

    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            assert_eq!(cached.biome_at(x, z), full.biome_at(x, z));
            assert_eq!(cached.surface_y(x, z), full.surface_y(x, z));
            assert_eq!(
                cached.heightmap_surface_y(x, z),
                full.heightmap_surface_y(x, z)
            );
        }
    }
    assert_eq!(cached.surf_range(), full.surf_range());
    assert_eq!(cached.content_top(), full.content_top());
    let (lo, hi) = full.surf_range();
    for cy in [lo.div_euclid(16), hi.div_euclid(16) + 1] {
        let sp = SectionPos::new(2, cy, -5);
        assert_eq!(
            generator
                .generate_section(sp, &full)
                .blocks_iter()
                .collect::<Vec<_>>(),
            generator
                .generate_section(sp, &cached)
                .blocks_iter()
                .collect::<Vec<_>>(),
            "cached column diverged at cy {cy}"
        );
    }
}

struct DeferringHooks {
    deferrals: AtomicUsize,
    terrain_dispatches: AtomicUsize,
    waits: AtomicUsize,
}

impl DeferringHooks {
    fn new(deferrals: usize) -> Arc<Self> {
        Arc::new(Self {
            deferrals: deferrals.into(),
            terrain_dispatches: 0.into(),
            waits: 0.into(),
        })
    }

    fn plan(pos: [i32; 3], column: i32, block: Block) -> FeatureOutcome {
        let origin = pos.map(|v| v * SECTION_SIZE as i32);
        FeatureOutcome::Plan(crate::hooks::GenerationPlan {
            blocks: vec![(
                [origin[0] + column, origin[1] + 8, origin[2] + 3],
                block.id(),
            )],
            ..Default::default()
        })
    }
}

impl GenHookDispatch for DeferringHooks {
    fn epoch(&self) -> u64 {
        u64::MAX
    }
    fn replaces(&self, _: WorldgenStage) -> bool {
        false
    }
    fn replace_climate(&self, _: &GenInputs) -> Option<Vec<u8>> {
        None
    }
    fn replace_terrain(&self, _: &GenInputs) -> Option<Vec<u16>> {
        None
    }
    fn replace_stage(&self, _: WorldgenStage, _: &GenInputs) -> FeatureOutcome {
        FeatureOutcome::Skipped
    }
    fn any_features_after(&self, stage: WorldgenStage) -> bool {
        matches!(stage, WorldgenStage::Terrain | WorldgenStage::Vegetation)
    }
    fn features_after(&self, stage: WorldgenStage) -> Vec<usize> {
        match stage {
            WorldgenStage::Terrain => vec![0],
            WorldgenStage::Vegetation => vec![1],
            _ => Vec::new(),
        }
    }
    fn dispatch_feature(&self, idx: usize, inputs: &GenInputs) -> FeatureOutcome {
        if idx == 0 {
            self.terrain_dispatches.fetch_add(1, Ordering::SeqCst);
            return Self::plan(inputs.section_pos, 1, Block::Glass);
        }
        let deferred = self
            .deferrals
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if deferred {
            FeatureOutcome::Deferred
        } else {
            Self::plan(inputs.section_pos, 2, Block::Stone)
        }
    }
    fn wait_deferred(&self) {
        self.waits.fetch_add(1, Ordering::SeqCst);
    }
}

fn blocks(section: &Section) -> Vec<u16> {
    section.blocks_iter().collect()
}

#[test]
fn a_deferred_section_resumes_at_the_deferring_hook() {
    let seed = 0xDEF0_0001;
    let reference = ChunkGenerator::with_hooks(seed, Some(DeferringHooks::new(0)));
    let col = reference.generate_column_gen(1, -2);
    let sp = SectionPos::new(1, col.surf_range().1.div_euclid(16), -2);
    let expected = match reference.start_section(sp, &col) {
        SectionGen::Ready(section) => blocks(&section),
        SectionGen::Deferred(_) => panic!("a hook that never defers deferred"),
    };

    let twice = DeferringHooks::new(2);
    let generator = ChunkGenerator::with_hooks(seed, Some(twice.clone()));
    let mut attempt = generator.start_section(sp, &col);
    let mut deferrals = 0;
    let section = loop {
        match attempt {
            SectionGen::Ready(section) => break section,
            SectionGen::Deferred(pending) => {
                assert_eq!(pending.pos(), sp);
                deferrals += 1;
                attempt = generator.resume_section(pending, &col);
            }
        }
    };
    assert_eq!(deferrals, 2);
    assert_eq!(
        twice.terrain_dispatches.load(Ordering::SeqCst),
        1,
        "resuming must not rerun the hooks before the deferral"
    );
    assert_eq!(
        twice.waits.load(Ordering::SeqCst),
        0,
        "the non-blocking path never waits"
    );
    assert_eq!(blocks(&section), expected);
    assert!(section.dirty);
}

#[test]
fn completing_a_deferred_section_waits_once_per_deferral() {
    let seed = 0xDEF0_0002;
    let reference = ChunkGenerator::with_hooks(seed, Some(DeferringHooks::new(0)));
    let col = reference.generate_column_gen(-3, 4);
    let sp = SectionPos::new(-3, col.surf_range().0.div_euclid(16), 4);
    let expected = blocks(&reference.generate_section(sp, &col));

    let thrice = DeferringHooks::new(3);
    let generator = ChunkGenerator::with_hooks(seed, Some(thrice.clone()));
    assert_eq!(blocks(&generator.generate_section(sp, &col)), expected);
    assert_eq!(thrice.waits.load(Ordering::SeqCst), 3);
    assert_eq!(thrice.terrain_dispatches.load(Ordering::SeqCst), 1);
}
