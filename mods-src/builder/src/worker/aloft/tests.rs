use super::*;

fn up(levels: i32) -> Outlook {
    Outlook {
        top: [0, levels, 0],
        climb: levels * LEVEL_TICKS,
        walkway: None,
        chain: 0,
        lays_from_here: true,
        moves: Vec::new(),
    }
}

#[test]
fn a_walkway_is_no_longer_than_a_climb_is_dear() {
    let tall = up(20);
    assert_eq!(longest_walkway(&tall, false, None), WALKWAY_LENGTH);
    let short = up(3);
    assert_eq!(
        longest_walkway(&short, false, None),
        3 * LEVEL_TICKS / WALKWAY_TICKS
    );
    assert_eq!(longest_walkway(&short, true, None), WALKWAY_LENGTH);
}

#[test]
fn a_walkway_must_beat_the_walk() {
    let tall = up(20);
    let walk = Some(([4, 20, 0], 2 * WALKWAY_TICKS));
    assert_eq!(longest_walkway(&tall, false, walk), 1);
    let stroll = Some(([1, 20, 0], WALKWAY_TICKS));
    assert_eq!(
        longest_walkway(&tall, false, stroll),
        0,
        "a walk this short always wins"
    );
}

#[test]
fn a_walkway_goes_on_only_from_its_end_and_within_its_chain() {
    let mut out = up(20);
    out.lays_from_here = false;
    assert_eq!(longest_walkway(&out, true, None), 0);
    out.lays_from_here = true;
    out.chain = WALKWAY_CHAIN - 2;
    assert_eq!(
        longest_walkway(&out, true, None),
        2,
        "the chain is nearly used up"
    );
}
