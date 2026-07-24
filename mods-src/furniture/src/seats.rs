//! Sitting: PURE MOD POLICY over the engine's actor-pose primitive
//! (`player_pose_set`). This module owns the seat layout (the [`PIECES`]
//! table — offsets in unrotated footprint space, like model geometry),
//! computes each seat's world anchor from the placed group's base + facing
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
//! The CLIENT instance mirrors the same always-claim as a predictor. One PASS:
//! a sneak click holding a placeable block defers to the placement consumer
//! (sneak-to-build against a chair, like the farming harvest's sneak rule).
//!
//! Breaking furniture is the one release this mod owes the engine (a pose is
//! not tied to any block): `block_broken` re-derives which former group the
//! cell belonged to and releases exactly the players anchored on its seats.

use mod_sdk::*;

use super::{held_places_a_block, Furniture};

/// One sit-able furniture piece: its block, the model footprint (mirror of
/// the pack's `models.json` `cells`), and its seats in unrotated footprint
/// space. A bench or sofa is one more row with more seats.
pub(super) struct Piece {
    pub(super) block: &'static str,
    pub(super) footprint: [u8; 3],
    pub(super) seats: &'static [[f32; 3]],
}

pub(super) const PIECES: &[Piece] = &[
    Piece {
        block: "furniture:chair",
        footprint: [1, 2, 1],
        seats: &[[0.5, -0.1, 0.25]],
    },
    Piece {
        block: "furniture:bench",
        footprint: [2, 2, 1],
        seats: &[[0.5, -0.1, 0.25], [1.5, -0.1, 0.25]],
    },
];

const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

/// A [`Piece`] with its block name resolved to the session id.
pub(super) struct ResolvedPiece {
    pub(super) block: BlockId,
    pub(super) piece: &'static Piece,
}

impl Furniture {
    pub(super) fn piece_for(&self, block: BlockId) -> Option<&'static Piece> {
        self.pieces
            .iter()
            .find(|p| p.block == block)
            .map(|p| p.piece)
    }

    /// Furniture consumer: seat the clicker in the free seat nearest the
    /// clicked cell (horizontal distance to the seat's world anchor, so the
    /// pick is facing-correct; declaration order breaks a tie). The claim is
    /// UNCONDITIONAL once the target is
    /// furniture — a fully occupied piece ABSORBS the click (see module docs)
    /// so the initiating client, which cannot see occupancy on its replica,
    /// never mispredicts a placement. One PASS: a sneak click holding a
    /// placeable block defers to the placement consumer (sneak-to-build).
    pub(super) fn try_sit(&self, pos: [i32; 3], player: PlayerId, actor: &PlayerSnapshot) -> bool {
        if actor.sneak && held_places_a_block(actor.held) {
            return false;
        }
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
        let (cx, cz) = (pos[0] as f32 + 0.5, pos[2] as f32 + 0.5);
        let mut free: Vec<(f32, [f32; 3])> = piece
            .seats
            .iter()
            .map(|seat| footprint_local_to_world(group.base, piece.footprint, group.facing, *seat))
            .filter(|anchor| !occupied.contains(anchor))
            .map(|anchor| ((anchor[0] - cx).powi(2) + (anchor[2] - cz).powi(2), anchor))
            .collect();
        free.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap()); // stable: ties keep declaration order
        for (_, anchor) in free {
            if player_pose_set(player, anchor, yaw, pose::SITTING) {
                break;
            }
        }
        true
    }

    /// CLIENT: gate-only mirror of [`Self::try_sit`] over a replica read.
    /// Furniture claims every click (seat or absorb), so the mirror is exact
    /// from the block id alone — no occupancy divergence is possible — apart
    /// from the same sneak+placeable pass the authoritative gate applies. A
    /// `None` replica cell never produces a claim.
    pub(super) fn predict_sit(&self, pos: [i32; 3], actor: &PlayerSnapshot) -> bool {
        if actor.sneak && held_places_a_block(actor.held) {
            return false;
        }
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
pub(super) fn release_broken_piece_sitters(block: BlockId, piece: &Piece, pos: [i32; 3]) {
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
                        let anchor = footprint_local_to_world(base, piece.footprint, facing, *seat);
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
