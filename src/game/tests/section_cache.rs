//! Cross-side contract tests for the section cache (WIKI/section-cache.md):
//! keep-shape unloads vouch content hashes and the client parks the evicted
//! copies; a re-entered section with unmoved content re-promotes as
//! `SectionCached` (byte-identical to what a full resend would install); a
//! belief hash that disagrees with current content forces the full payload;
//! and a client that declined to park heals through `SectionCacheMiss`.
//!
//! The fixture leans on GENERATED terrain (no save): leaving evicts the area
//! server-side and returning regenerates it deterministically, so untouched
//! sections hash equal across the trip — the same property that makes the
//! cache effective in real play.

use super::super::tick::TICK_DT;
use super::common::{game, TestGame};
use crate::chunk::{ChunkPos, SectionPos};
use crate::game::GameInput;
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::{SectionCacheClaim, SectionPayload, ServerToClient};

const HOME: Vec3 = Vec3::new(8.5, 80.0, 8.5);
const FAR: Vec3 = Vec3::new(328.5, 80.0, 328.5);
const HOME_COLUMN: ChunkPos = ChunkPos { cx: 0, cz: 0 };

fn place_player(game: &mut TestGame, feet: Vec3) {
    game.player.pos = feet;
    game.player.vel = Vec3::ZERO;
    game.server.sessions[0].player.pos = feet;
    game.server.sessions[0].player.vel = Vec3::ZERO;
}

fn frame(game: &mut TestGame) -> Vec<ServerToClient> {
    let msgs = game.tick_recorded(TICK_DT, &GameInput::default());
    std::thread::sleep(std::time::Duration::from_millis(1));
    msgs
}

/// Pump frames until `done`, panicking past the deadline with a summary of
/// what flowed; returns every recorded server→client message.
fn frames_until(
    game: &mut TestGame,
    what: &str,
    done: impl Fn(&TestGame) -> bool,
) -> Vec<ServerToClient> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut recorded = Vec::new();
    while !done(game) {
        if std::time::Instant::now() >= deadline {
            panic!("timed out: {what}; recorded {:?}", kind_counts(&recorded));
        }
        recorded.extend(frame(game));
    }
    recorded
}

fn kind_counts(msgs: &[ServerToClient]) -> std::collections::BTreeMap<&'static str, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for m in msgs {
        *counts.entry(kind(m)).or_default() += 1;
    }
    counts
}

fn kind(m: &ServerToClient) -> &'static str {
    match m {
        ServerToClient::ColumnData(_) => "ColumnData",
        ServerToClient::SectionData(_) => "SectionData",
        ServerToClient::SectionCached { .. } => "SectionCached",
        ServerToClient::SectionUnload { .. } => "SectionUnload",
        ServerToClient::ColumnUnload { .. } => "ColumnUnload",
        ServerToClient::LightData(_) => "LightData",
        ServerToClient::Tick(_) => "Tick",
        _ => "other",
    }
}

/// Pump until the home column replicated AND no section payloads
/// (`SectionData`/`SectionCached`) arrived for a stretch of frames — the
/// terrain of an initial load (or a re-entry) has landed. Light rebakes and
/// tick deltas may keep trickling (water settling); they don't gate this.
fn settle(game: &mut TestGame, what: &str) -> Vec<ServerToClient> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut recorded = Vec::new();
    let mut quiet = 0;
    loop {
        let msgs = frame(game);
        let sections = msgs
            .iter()
            .any(|m| matches!(kind(m), "SectionData" | "SectionCached"));
        quiet = if sections { 0 } else { quiet + 1 };
        recorded.extend(msgs);
        if quiet >= 30
            && game.replica.chunk_loaded(0, 0)
            && !home_column_payloads(game).is_empty()
        {
            return recorded;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out settling: {what}; recorded {:?}", kind_counts(&recorded));
        }
    }
}

/// Snapshot every loaded replica section of the home column, keyed by pos.
fn home_column_payloads(game: &TestGame) -> Vec<(SectionPos, SectionPayload)> {
    (-4..16)
        .filter_map(|cy| {
            let sp = SectionPos::new(HOME_COLUMN.cx, cy, HOME_COLUMN.cz);
            game.replica.section_payload(sp).map(|p| (sp, p))
        })
        .collect()
}

fn leave(game: &mut TestGame) -> Vec<ServerToClient> {
    place_player(game, FAR);
    frames_until(game, "the home column unloaded", |g| {
        !g.replica.chunk_loaded(0, 0)
    })
}

fn return_home(game: &mut TestGame) -> Vec<ServerToClient> {
    place_player(game, HOME);
    settle(game, "re-entry streaming")
}

#[test]
fn unmoved_sections_repromote_from_the_cache_byte_identically() {
    let mut game = game();
    place_player(&mut game, HOME);
    settle(&mut game, "initial load");

    let unloads = leave(&mut game);
    let vouched: Vec<SectionPos> = unloads
        .iter()
        .filter_map(|m| match m {
            ServerToClient::ColumnUnload { pos, cache_hashes } if *pos == HOME_COLUMN => {
                Some(cache_hashes.iter().map(|&(cy, _)| SectionPos::new(pos.cx, cy, pos.cz)))
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        !vouched.is_empty(),
        "the column unload vouched its live sections"
    );
    for sp in &vouched {
        assert!(
            game.section_cache.contains(*sp),
            "vouched section {sp:?} parked client-side"
        );
    }

    let msgs = return_home(&mut game);
    let cached: Vec<SectionPos> = msgs
        .iter()
        .filter_map(|m| match m {
            ServerToClient::SectionCached { pos, .. } => Some(*pos),
            _ => None,
        })
        .filter(|sp| sp.chunk_pos() == HOME_COLUMN)
        .collect();
    assert!(
        !cached.is_empty(),
        "untouched regenerated sections re-promoted from the cache; flow: {:?}",
        kind_counts(&msgs)
    );
    for sp in &cached {
        assert!(
            !msgs.iter().any(
                |m| matches!(m, ServerToClient::SectionData(s) if s.pos == *sp)
            ),
            "a re-promoted section {sp:?} was not also re-streamed"
        );
        // The contract: the re-promoted replica copy equals what a full send
        // would install RIGHT NOW. The loopback pipe skips the id remap, so
        // the two payloads compare directly; the harness is synchronous, so
        // nothing mutates between the two reads.
        let client = game
            .replica
            .section_payload(*sp)
            .expect("re-promoted section is live client-side");
        let server = game
            .server
            .world
            .section_payload(*sp)
            .expect("re-entered section is loaded server-side");
        assert_eq!(
            client, server,
            "re-promotion of {sp:?} equals a fresh full send"
        );
    }
}

#[test]
fn a_moved_belief_hash_resends_the_full_payload() {
    let mut game = game();
    place_player(&mut game, HOME);
    settle(&mut game, "initial load");
    let unloads = leave(&mut game);
    let &(cy, hash) = unloads
        .iter()
        .find_map(|m| match m {
            ServerToClient::ColumnUnload { pos, cache_hashes } if *pos == HOME_COLUMN => {
                cache_hashes.first()
            }
            _ => None,
        })
        .expect("the home column unload vouched at least one section");
    let sp = SectionPos::new(HOME_COLUMN.cx, cy, HOME_COLUMN.cz);

    // Stand in for "the content changed while the client was away": force the
    // belief to a hash current content can never equal (a real edit moves the
    // CURRENT hash instead — the same inequality drives the same branch).
    game.server.sessions[0]
        .terrain
        .seed_client_cache(&[SectionCacheClaim {
            pos: sp,
            hash: hash.wrapping_add(1),
        }]);

    let msgs = return_home(&mut game);
    let (cached, full) = (
        msgs.iter()
            .filter(|m| matches!(m, ServerToClient::SectionCached { pos, .. } if *pos == sp))
            .count(),
        msgs.iter()
            .filter(|m| matches!(m, ServerToClient::SectionData(s) if s.pos == sp))
            .count(),
    );
    assert_eq!(
        (cached, full),
        (0, 1),
        "a belief that disagrees with current content ships the full payload"
    );
    assert!(
        !game.section_cache.contains(sp),
        "the full payload superseded (discarded) the stale parked copy"
    );
}

#[test]
fn a_pending_prediction_declines_parking_and_heals_by_cache_miss() {
    let mut game = game();
    place_player(&mut game, HOME);
    settle(&mut game, "initial load");

    // Pending predicted edits across the whole home column at unload time:
    // the replica copies may not equal what the server vouches, so the
    // client must not park them (reach makes this impossible in real play —
    // the guard is the backstop). The server still believes they parked.
    for cy in -4..16 {
        game.game
            .predicted_presentation_cells
            .insert(IVec3::new(8, cy * 16 + 8, 8));
    }
    let unloads = leave(&mut game);
    let vouched: Vec<SectionPos> = unloads
        .iter()
        .filter_map(|m| match m {
            ServerToClient::ColumnUnload { pos, cache_hashes } if *pos == HOME_COLUMN => {
                Some(cache_hashes.iter().map(|&(cy, _)| SectionPos::new(pos.cx, cy, pos.cz)))
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert!(!vouched.is_empty(), "the unload still vouched (server side)");
    for sp in &vouched {
        assert!(
            !game.section_cache.contains(*sp),
            "a section with pending predictions never parks ({sp:?})"
        );
    }
    game.game.predicted_presentation_cells.clear();

    let msgs = return_home(&mut game);
    let healed = vouched.iter().any(|sp| {
        msgs.iter()
            .any(|m| matches!(m, ServerToClient::SectionCached { pos, .. } if pos == sp))
            && msgs
                .iter()
                .any(|m| matches!(m, ServerToClient::SectionData(s) if s.pos == *sp))
    });
    assert!(
        healed,
        "belief drift = SectionCached miss, then the healing full payload; flow: {:?}",
        kind_counts(&msgs)
    );
}
