use super::*;
use crate::{StructureRequirementData, TerrainSpace};

fn info(regions: Vec<StructureRequirementData>) -> StructureInfoData {
    StructureInfoData {
        bounds: [([0; 3], [0; 3]); 4],
        connectors: Vec::new(),
        requirements: regions,
    }
}

#[test]
fn terrain_probes_rotate_the_complete_region_without_section_clipping() {
    let info = info(vec![StructureRequirementData {
        min: [-2, -1, 1],
        max: [1, 1, 2],
        space: TerrainSpace::Air,
    }]);
    let mut placement = StructurePlacement {
        template: "fixture:room".into(),
        origin: [15, -16, -1],
        turn: 0,
    };
    let base = structure_probes(&placement, &info).unwrap();
    assert_eq!(base.len(), 24);
    for turn in 0..4 {
        placement.turn = turn;
        let probes = structure_probes(&placement, &info).unwrap();
        for ((pos, space), (unrotated, _)) in probes.iter().zip(&base) {
            let local = std::array::from_fn(|i| unrotated[i] - placement.origin[i]);
            let rotated = rotate(local, turn).unwrap();
            assert_eq!(
                *pos,
                std::array::from_fn(|i| rotated[i] + placement.origin[i])
            );
            assert_eq!(*space, TerrainSpace::Air);
        }
    }
}

#[test]
fn invalid_or_excessive_probes_fail_before_expansion() {
    let region = StructureRequirementData {
        min: [0; 3],
        max: [15; 3],
        space: TerrainSpace::Solid,
    };
    let mut placement = StructurePlacement {
        template: "fixture:room".into(),
        origin: [0; 3],
        turn: 0,
    };
    assert!(structure_probes(&placement, &info(vec![region.clone(); 2])).is_none());
    let mut reversed = region.clone();
    reversed.min[0] = 16;
    assert!(structure_probes(&placement, &info(vec![reversed])).is_none());
    placement.origin[0] = i32::MAX;
    assert!(structure_probes(&placement, &info(vec![region.clone()])).is_none());
    placement.turn = 4;
    assert!(structure_probes(&placement, &info(vec![region])).is_none());
}
