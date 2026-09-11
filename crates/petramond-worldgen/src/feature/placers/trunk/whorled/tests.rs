use super::*;
use crate::feature::VoxelSink;
use std::collections::{BTreeSet, VecDeque};

#[derive(Default)]
struct Wood(BTreeSet<[i32; 3]>);
impl VoxelSink for Wood {
    fn get(&self, p: IVec3) -> Block {
        if self.0.contains(&p.to_array()) {
            Block::OakLog
        } else {
            Block::Air
        }
    }
    fn set(&mut self, p: IVec3, _: Block) {
        self.0.insert(p.to_array());
    }
}

#[test]
fn whorls_remain_connected_and_translate_across_negative_coordinates() {
    let trunk = WhorledTrunk {
        limbs: (6, 8),
        band: (0.2, 0.9),
        reach: (3, 5),
        rise: (1, 3),
        flare: 2,
        stem_fraction: 1.0,
        elbow_rise: 0.3,
        limb_radius: 1,
    };
    trunk.validate((20, 26)).unwrap();
    for seed in 1..=12 {
        let mut baseline = Wood::default();
        let plan = trunk.place(
            &mut FeatureCtx::new(&mut baseline),
            IVec3::ZERO,
            (20, 26),
            Block::OakLog,
            &mut FeatureRng::from_state(seed),
        );
        let mut seen = BTreeSet::from([[0, 0, 0]]);
        let mut queue = VecDeque::from([IVec3::ZERO]);
        while let Some(p) = queue.pop_front() {
            for step in petramond_world::mathh::FACE_NEIGHBORS {
                let next = p + step;
                if baseline.0.contains(&next.to_array()) && seen.insert(next.to_array()) {
                    queue.push_back(next);
                }
            }
        }
        assert_eq!(
            seen, baseline.0,
            "all limbs and roots must be face-connected"
        );
        assert!(plan.attach.iter().all(|p| seen.contains(&p.to_array())));
        let mut anchored = BTreeSet::new();
        assert!(trunk.is_anchored(
            &mut |x, z| {
                anchored.insert([x, 0, z]);
                -1
            },
            IVec3::ZERO,
        ));
        let ground: BTreeSet<_> = baseline.0.iter().copied().filter(|p| p[1] == 0).collect();
        assert_eq!(
            anchored, ground,
            "support probes must cover exactly the generated roots"
        );
        for origin in [IVec3::new(-17, -33, -7), IVec3::new(19, 65, 23)] {
            let mut translated = Wood::default();
            trunk.place(
                &mut FeatureCtx::new(&mut translated),
                origin,
                (20, 26),
                Block::OakLog,
                &mut FeatureRng::from_state(seed),
            );
            let normalized: BTreeSet<_> = translated
                .0
                .into_iter()
                .map(|p| (IVec3::from(p) - origin).to_array())
                .collect();
            assert_eq!(normalized, baseline.0);
        }
    }
}
