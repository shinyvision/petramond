//! Sitting: pure mod policy over the engine's actor-pose primitive (`player_pose_set`).
//! Block row data owns seat layout (offsets in unrotated footprint space, like model geometry).
//! We compute each seat's world anchor from the placed group's base + facing
//! (`block_model_group` + `footprint_local_to_world`), and read occupancy straight from the
//! engine roster (`pose_anchor`) — never mirrored mod state, so nothing to desync or clean up.
//! Engine owns the mechanism: one pose per player, no two players on one exact anchor,
//! replication, seated body, release valves (sneak / death / spectator / leave).
//!
//! A click on furniture is always claimed — seated, or absorbed once every seat is taken.
//! The absorb is deliberate: occupancy is invisible to the initiating client's replica, so
//! pass-when-full would let the client ghost a block placement the server then refuses.
//! Client mirrors the same always-claim as a predictor. One exception: a sneak click while
//! holding a placeable block defers to the placement consumer (sneak-to-build against a
//! chair, same as farming's harvest sneak rule).
//!
//! Breaking furniture is the one release this mod owes the engine (a pose isn't tied to any
//! block): `block_broken` figures out which former group the cell belonged to and releases
//! just the players anchored on its seats.

use mod_sdk::*;

use super::{keys, Furniture};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Piece {
    pub(super) footprint: [u8; 3],
    pub(super) seats: Vec<[f32; 3]>,
}

pub(super) fn load_pieces() -> Vec<ResolvedPiece> {
    blocks_with_data_as::<Piece>(keys::SEATS)
        .into_iter()
        .filter_map(|(block, piece)| {
            if piece.footprint.contains(&0) || piece.seats.is_empty() {
                log(&format!(
                    "furniture: seat row for {block:?} has no footprint or seats"
                ));
                None
            } else {
                Some(ResolvedPiece { block, piece })
            }
        })
        .collect()
}

const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

pub(super) struct ResolvedPiece {
    pub(super) block: BlockId,
    pub(super) piece: Piece,
}

impl Furniture {
    pub(super) fn piece_for(&self, block: BlockId) -> Option<&Piece> {
        self.pieces
            .iter()
            .find(|p| p.block == block)
            .map(|p| &p.piece)
    }

    pub(super) fn try_sit(&self, pos: [i32; 3], player: PlayerId, actor: &PlayerSnapshot) -> bool {
        let Some(piece) = self.seat_gate(&SideWorld::Server, pos, actor) else {
            return false;
        };
        let Some(group) = block_model_group(pos) else {
            return false;
        };
        let occupied: Vec<[f64; 3]> = players()
            .into_iter()
            .filter_map(|p| p.state.pose_anchor)
            .collect();
        let yaw = facing_player_yaw(group.facing);
        let (cx, cz) = (pos[0] as f64 + 0.5, pos[2] as f64 + 0.5);
        let mut free: Vec<(f64, [f64; 3])> = piece
            .seats
            .iter()
            .map(|seat| footprint_local_to_world(group.base, piece.footprint, group.facing, *seat))
            .filter(|anchor| !occupied.contains(anchor))
            .map(|anchor| ((anchor[0] - cx).powi(2) + (anchor[2] - cz).powi(2), anchor))
            .collect();
        free.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (_, anchor) in free {
            if player_pose_set(player, anchor, yaw, pose::SITTING) {
                break;
            }
        }
        true
    }

    /// The seat consumer's claim GATE, run by both instances over their own
    /// [`WorldView`]: furniture claims every click (seat or absorb), so the
    /// gate is the block id alone — no occupancy read, so the client's
    /// prediction is exact — apart from the sneak+placeable pass. Answers
    /// the clicked piece; a cell the instance cannot read never claims.
    pub(super) fn seat_gate(
        &self,
        world: &impl WorldView,
        pos: [i32; 3],
        actor: &PlayerSnapshot,
    ) -> Option<&Piece> {
        if actor.sneak && held_item_places_block(actor.held) {
            return None;
        }
        world.block(pos).and_then(|b| self.piece_for(b))
    }
}

/// Releases every player still posed on the seats of the group the broken cell belonged to. The
/// group is gone, so we guess its base and facing: each (facing, contained cell) pair gives a
/// candidate base. If that base still holds the piece, it's a different group that's still standing
/// (an adjacent chair) and we skip it. The rest have their exact seat anchors matched against the
/// roster. Anchors are exact, from the same f32 math as sitting, so a neighbouring piece's
/// sitter is never released by proximity.
pub(super) fn release_broken_piece_sitters(block: BlockId, piece: &Piece, pos: [i32; 3]) {
    let posed: Vec<(PlayerId, [f64; 3])> = players()
        .into_iter()
        .filter_map(|p| p.state.pose_anchor.map(|a| (p.id, a)))
        .collect();
    if posed.is_empty() {
        return;
    }
    let [sx, sy, sz] = piece.footprint;
    for facing in FACINGS {
        let (wx, wz) = match facing {
            Facing::North | Facing::South => (sx, sz),
            Facing::East | Facing::West => (sz, sx),
        };
        for dx in 0..wx as i32 {
            for dy in 0..sy as i32 {
                for dz in 0..wz as i32 {
                    let base = [pos[0] - dx, pos[1] - dy, pos[2] - dz];
                    if get_block(base) == Some(block) {
                        continue;
                    }
                    for &seat in &piece.seats {
                        let anchor = footprint_local_to_world(base, piece.footprint, facing, seat);
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

#[cfg(test)]
mod row_tests {
    use super::*;

    #[test]
    fn shipped_seat_rows_have_usable_footprints() {
        let rows = pack_rows_with_data(include_str!("../pack/blocks.json"), "blocks", keys::SEATS);
        for (block, raw) in rows {
            let piece: Piece = parse_row_data(&raw).unwrap_or_else(|e| panic!("{block}: {e}"));
            assert!(piece.footprint.iter().all(|side| *side > 0), "{block}");
            assert!(!piece.seats.is_empty(), "{block}");
            assert!(
                piece.seats.iter().flatten().all(|v| v.is_finite()),
                "{block}"
            );
        }
    }
}
