//! furniture — craftable wooden furniture as bbmodel blocks.
//!
//! Sitting is PURE MOD POLICY over the engine's actor-pose primitive
//! (`player_pose_set`): this mod owns the seat layout (the [`PIECES`] table —
//! offsets in unrotated footprint space, like model geometry), computes each
//! seat's world anchor from the placed group's base + facing
//! (`block_model_group` + `footprint_local_to_world`), and derives occupancy
//! from the engine roster (`pose_anchor`) — never from mirrored mod state, so
//! there is nothing to desync or clean up. The engine owns mechanism: one
//! pose per player, no two players on one exact anchor, replication, the
//! seated body, and the release valves (sneak / death / spectator / leave).
//!
//! A click on furniture is ALWAYS claimed — seated, or ABSORBED when every
//! seat is taken. The absorb is deliberate (an interact-doctrine exception):
//! occupancy is invisible to the initiating client's replica, so a pass-when-
//! full would let the client ghost a block placement the server then refuses.
//! The CLIENT instance mirrors the same always-claim as a predictor.
//!
//! Breaking furniture is the one release this mod owes the engine (a pose is
//! not tied to any block): `block_broken` re-derives which former group the
//! cell belonged to and releases exactly the players anchored on its seats.

use mod_sdk::*;

const ON_INTERACT: u32 = 1;
const ON_BLOCK_BROKEN: u32 = 2;

/// One sit-able furniture piece: its block, the model footprint (mirror of
/// the pack's `models.json` `cells`), and its seats in unrotated footprint
/// space. A bench or sofa is one more row with more seats.
struct Piece {
    block: &'static str,
    footprint: [u8; 3],
    seats: &'static [[f32; 3]],
}

const PIECES: &[Piece] = &[Piece {
    block: "furniture:chair",
    footprint: [1, 2, 1],
    seats: &[[0.5, -0.1, 0.25]],
}];

const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

/// A [`Piece`] with its block name resolved to the session id.
struct ResolvedPiece {
    block: BlockId,
    piece: &'static Piece,
}

#[derive(Default)]
struct Furniture {
    pieces: Vec<ResolvedPiece>,
    /// Running as the CLIENT instance: only PREDICT the sit claim against the
    /// replica — sim host calls are unavailable on this side.
    client: bool,
}

impl Mod for Furniture {
    fn init(&mut self) {
        self.pieces = PIECES
            .iter()
            .filter_map(|piece| {
                let block = resolve_block(piece.block)?;
                Some(ResolvedPiece { block, piece })
            })
            .collect();
        self.client = runtime_side() == RuntimeSide::Client;
        register_event_handler(EventKind::InteractAttempt, 0, ON_INTERACT);
        if !self.client {
            register_event_handler(EventKind::BlockBroken, 0, ON_BLOCK_BROKEN);
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (
                ON_INTERACT,
                EventPayload::InteractAttempt {
                    block: Some(pos),
                    player,
                    ..
                },
            ) => {
                let claimed = if self.client {
                    self.predict_sit(*pos)
                } else {
                    self.try_sit(*pos, *player)
                };
                if claimed {
                    Outcome::Cancel
                } else {
                    Outcome::Continue
                }
            }
            (ON_BLOCK_BROKEN, EventPayload::BlockBroken { pos, block, .. }) => {
                if let Some(resolved) = self.pieces.iter().find(|p| p.block == *block) {
                    release_broken_piece_sitters(resolved.block, resolved.piece, *pos);
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }
}

impl Furniture {
    fn piece_for(&self, block: BlockId) -> Option<&'static Piece> {
        self.pieces
            .iter()
            .find(|p| p.block == block)
            .map(|p| p.piece)
    }

    /// Furniture consumer: seat the clicker in the first unoccupied seat of
    /// the clicked piece. The claim is UNCONDITIONAL once the target is
    /// furniture — a fully occupied piece ABSORBS the click (see module docs)
    /// so the initiating client, which cannot see occupancy on its replica,
    /// never mispredicts a placement.
    fn try_sit(&self, pos: [i32; 3], player: PlayerId) -> bool {
        let Some(piece) = get_block(pos).and_then(|b| self.piece_for(b)) else {
            return false;
        };
        let Some(group) = block_model_group(pos) else {
            return false; // frozen/inconsistent state: never claim
        };
        let occupied: Vec<[f32; 3]> = players()
            .into_iter()
            .filter_map(|p| p.state.pose_anchor)
            .collect();
        let yaw = facing_player_yaw(group.facing);
        for seat in piece.seats {
            let anchor = footprint_local_to_world(group.base, piece.footprint, group.facing, *seat);
            if occupied.contains(&anchor) {
                continue;
            }
            if player_pose_set(player, anchor, yaw, pose::SITTING) {
                break;
            }
        }
        true
    }

    /// CLIENT: gate-only mirror of [`Self::try_sit`] over a replica read.
    /// Furniture claims every click (seat or absorb), so the mirror is exact
    /// from the block id alone — no occupancy divergence is possible. A
    /// `None` replica cell never produces a claim.
    fn predict_sit(&self, pos: [i32; 3]) -> bool {
        client_blocks_at(vec![pos])
            .into_iter()
            .next()
            .flatten()
            .is_some_and(|b| self.piece_for(b).is_some())
    }
}

/// Release every player still posed on the seats of the group the broken
/// cell belonged to. The group is gone, so its base/facing are re-derived by
/// HYPOTHESIS: every (facing, contained-cell) pair yields a candidate base;
/// a candidate whose base still holds the piece is a different, still-
/// standing group (an adjacent chair) and is skipped; the rest have their
/// exact seat anchors matched against the roster. Anchors are bit-exact
/// (same f32 pipeline as the sit), so equality is sound and a neighbouring
/// piece's sitter can never be released by proximity.
fn release_broken_piece_sitters(block: BlockId, piece: &Piece, pos: [i32; 3]) {
    let posed: Vec<(PlayerId, [f32; 3])> = players()
        .into_iter()
        .filter_map(|p| p.state.pose_anchor.map(|a| (p.id, a)))
        .collect();
    if posed.is_empty() {
        return;
    }
    let [sx, sy, sz] = piece.footprint;
    for facing in FACINGS {
        // The rotated footprint's world extent: X/Z swap for East/West.
        let (wx, wz) = match facing {
            Facing::North | Facing::South => (sx, sz),
            Facing::East | Facing::West => (sz, sx),
        };
        for dx in 0..wx as i32 {
            for dy in 0..sy as i32 {
                for dz in 0..wz as i32 {
                    let base = [pos[0] - dx, pos[1] - dy, pos[2] - dz];
                    if get_block(base) == Some(block) {
                        continue; // a still-standing group owns this base
                    }
                    for seat in piece.seats {
                        let anchor =
                            footprint_local_to_world(base, piece.footprint, facing, *seat);
                        for (id, a) in &posed {
                            if *a == anchor {
                                mob_dismount(*id);
                            }
                        }
                    }
                }
            }
        }
    }
}

mod_sdk::register_mod!(Furniture);
