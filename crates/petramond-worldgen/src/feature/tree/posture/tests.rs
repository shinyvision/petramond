use super::*;

#[test]
fn zero_lean_keeps_the_whole_stem_upright_even_with_a_shoulder() {
    for direction in CARDINALS {
        for shoulder in -1..=1 {
            let posture = TrunkPosture {
                direction,
                lean: 0,
                shoulder,
            };
            for height in [5, 12, 27] {
                for level in 0..height {
                    assert_eq!(posture.offset(level, height), (0, 0));
                }
            }
        }
    }
}
