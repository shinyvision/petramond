use super::*;

fn p(x: i32) -> IVec3 {
    IVec3::new(x, 0, 0)
}

/// Two readers see the same announcements independently: one draining does
/// not consume the other's view, and each sees a position exactly once.
#[test]
fn readers_drain_independently() {
    let mut feed = ChangeFeed::default();
    feed.push(p(1));
    assert_eq!(feed.drain(ChangeReader::Mobs), (vec![p(1)], false));
    feed.push(p(2));
    assert_eq!(feed.drain(ChangeReader::Items), (vec![p(1), p(2)], false));
    assert_eq!(feed.drain(ChangeReader::Mobs), (vec![p(2)], false));
    assert_eq!(feed.drain(ChangeReader::Mobs), (vec![], false));
    assert_eq!(feed.drain(ChangeReader::Items), (vec![], false));
}

/// An overflow reaches EVERY reader once, however late it drains; the
/// buffer stays bounded while nobody drains; and a reader that lags never
/// costs a current one its exact positions.
#[test]
fn overflow_is_reported_once_per_reader() {
    let mut feed = ChangeFeed::default();
    let _ = feed.drain(ChangeReader::Mobs);
    for i in 0..(CHANGE_FEED_CAP as i32 * 3) {
        feed.push(p(i));
    }
    assert!(feed.window.len() <= CHANGE_FEED_CAP);
    let (_, overflow) = feed.drain(ChangeReader::Mobs);
    assert!(overflow, "positions were lost since the last drain");
    feed.push(p(-1));
    assert_eq!(feed.drain(ChangeReader::Mobs), (vec![p(-1)], false));
    let (_, overflow) = feed.drain(ChangeReader::Items);
    assert!(overflow, "the late reader still learns of the overflow");
    assert_eq!(feed.drain(ChangeReader::Items), (vec![], false));
}
