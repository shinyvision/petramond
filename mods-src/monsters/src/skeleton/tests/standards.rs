use crate::post_marker::CampBox;
use crate::skeleton::standards::pieces;

#[test]
fn a_camp_is_searched_in_pieces_that_cover_it_once_within_the_search_cap() {
    let camp = CampBox {
        min: [-37, 58, 100],
        max: [33, 99, 131],
    };
    let pieces = pieces(camp);
    let mut seen = std::collections::BTreeSet::new();
    for piece in &pieces {
        let size = [0, 1, 2].map(|a| i64::from(piece.max[a] - piece.min[a] + 1));
        assert!(size.iter().product::<i64>() <= mod_sdk::FIND_BLOCKS_VOLUME_MAX);
        for x in piece.min[0]..=piece.max[0] {
            for y in piece.min[1]..=piece.max[1] {
                for z in piece.min[2]..=piece.max[2] {
                    assert!(camp.contains([x, y, z]), "{piece:?} reaches past the camp");
                    assert!(seen.insert([x, y, z]), "{piece:?} searches a cell twice");
                }
            }
        }
    }
    let volume = [0, 1, 2].map(|a| (camp.max[a] - camp.min[a] + 1) as usize);
    assert_eq!(seen.len(), volume.iter().product::<usize>());
}
