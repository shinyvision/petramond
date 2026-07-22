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
//! The CLIENT instance mirrors the same always-claim as a predictor. One PASS:
//! a sneak click holding a placeable block defers to the placement consumer
//! (sneak-to-build against a chair, like the farming harvest's sneak rule).
//!
//! Breaking furniture is the one release this mod owes the engine (a pose is
//! not tied to any block): `block_broken` re-derives which former group the
//! cell belonged to and releases exactly the players anchored on its seats.
//!
//! Chains are three single-cell rows — `furniture:chain` (vertical, the
//! item-linked base), `furniture:chain_ns`, `furniture:chain_ew` — sharing
//! ONE Layer-3 custom shape (`shapes.json` + the bakes below); the axis is
//! block IDENTITY (the ladder-row pattern), so the bake orients each cell
//! from its block id alone and placement needs no per-cell state. The
//! placement plan picks the sibling row from the clicked face's normal and
//! returns it as the plan's block override. Placement is fully
//! deterministic, so the ENGINE predicts it whole: the client runs the same
//! plan + gates against its replica and ghosts the exact write — the mod
//! ships no placement predictor of its own.

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

const PIECES: &[Piece] = &[
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

/// The chain family: the shared shape-kind id and its three axis rows —
/// vertical (the item-linked base), north/south, east/west.
struct Chains {
    shape: u8,
    rows: [BlockId; 3],
}

/// The plate pair per axis (matching `Chains::rows`): two crossing
/// 2/16-thick, 3/16-wide plates — the vanilla chain geometry. ONE geometry
/// source for the sim and render bakes so collision, selection, and the
/// drawn boxes can't drift; the mesher alpha-cuts the plates out of the
/// row's link tiles.
const PLATES: [[ShapeAabb; 2]; 3] = [
    // vertical
    [
        ShapeAabb {
            min: [6.5 / 16.0, 0.0, 7.0 / 16.0],
            max: [9.5 / 16.0, 1.0, 9.0 / 16.0],
        },
        ShapeAabb {
            min: [7.0 / 16.0, 0.0, 6.5 / 16.0],
            max: [9.0 / 16.0, 1.0, 9.5 / 16.0],
        },
    ],
    // north/south
    [
        ShapeAabb {
            min: [6.5 / 16.0, 7.0 / 16.0, 0.0],
            max: [9.5 / 16.0, 9.0 / 16.0, 1.0],
        },
        ShapeAabb {
            min: [7.0 / 16.0, 6.5 / 16.0, 0.0],
            max: [9.0 / 16.0, 9.5 / 16.0, 1.0],
        },
    ],
    // east/west
    [
        ShapeAabb {
            min: [0.0, 6.5 / 16.0, 7.0 / 16.0],
            max: [1.0, 9.5 / 16.0, 9.0 / 16.0],
        },
        ShapeAabb {
            min: [0.0, 7.0 / 16.0, 6.5 / 16.0],
            max: [1.0, 9.0 / 16.0, 9.5 / 16.0],
        },
    ],
];

impl Chains {
    /// The row for a clicked face's normal: top/bottom hangs a vertical
    /// chain, a side face lays it along that face's axis (vanilla rule).
    fn row_for_normal(&self, n: [i32; 3]) -> BlockId {
        if n[1] != 0 {
            self.rows[0]
        } else if n[0] != 0 {
            self.rows[2]
        } else {
            self.rows[1]
        }
    }

    /// The plate pair for a placed chain cell, oriented by its block id
    /// (the bake's whole orientation input — a pure function of the cell).
    fn plates_for(&self, block: BlockId) -> Vec<ShapeAabb> {
        let axis = self.rows.iter().position(|&r| r == block).unwrap_or(0);
        PLATES[axis].to_vec()
    }
}

/// A [`Piece`] with its block name resolved to the session id.
struct ResolvedPiece {
    block: BlockId,
    piece: &'static Piece,
}

#[derive(Default)]
struct Furniture {
    pieces: Vec<ResolvedPiece>,
    chains: Option<Chains>,
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
        self.chains = resolve_chains();
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
                let actor = player_state();
                let claimed = if self.client {
                    self.predict_sit(*pos, &actor)
                } else {
                    self.try_sit(*pos, *player, &actor)
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

    /// Chain SIM bake (deterministic — server and client replica): the plate
    /// pair for the cell's axis, oriented by its block id alone (a pure
    /// function of the cell, per the bake purity rule). Light passes — the
    /// plates are thin, so every cell reports the open aperture.
    fn bake_shape_sim(&mut self, shape_kind: u8, cells: &[CellInput]) -> Vec<BakedSimCell> {
        let Some(chains) = self.chains.as_ref().filter(|c| c.shape == shape_kind) else {
            return Vec::new();
        };
        cells
            .iter()
            .map(|cell| BakedSimCell {
                collision_boxes: chains.plates_for(cell.block_id),
                light_aperture: LightAperture::Open,
            })
            .collect()
    }

    /// Chain RENDER bake (client): the same plates the sim bake reports, so
    /// the drawn boxes, the selection union, and the collision agree.
    fn bake_shape_render(&mut self, shape_kind: u8, cells: &[CellInput]) -> Vec<BakedRenderCell> {
        let Some(chains) = self.chains.as_ref().filter(|c| c.shape == shape_kind) else {
            return Vec::new();
        };
        cells
            .iter()
            .map(|cell| BakedRenderCell {
                boxes: chains.plates_for(cell.block_id),
            })
            .collect()
    }

    /// Chain ITEM bake (client, once at load): the icon / in-hand / dropped
    /// form is always the VERTICAL plate pair, however the block row the
    /// item links is oriented — a held chain reads like the vanilla item.
    fn bake_shape_item(&mut self, shape_kind: u8, _block: BlockId) -> BakedItemGeometry {
        let boxes = match self.chains.as_ref().filter(|c| c.shape == shape_kind) {
            Some(_) => PLATES[0].to_vec(),
            None => Vec::new(),
        };
        BakedItemGeometry { boxes }
    }

    /// Chain placement: accept the click cell and write the axis row for the
    /// clicked face's normal (vertical off the top/bottom faces, north/south
    /// or east/west off the side faces) as the plan's block override. The
    /// host owns every world gate — loaded, replaceable, body occupancy.
    fn shape_placement_plan(
        &mut self,
        shape_kind: u8,
        _block: BlockId,
        inputs: &PlaceInputsView,
    ) -> ShapePlacementResult {
        let row = self
            .chains
            .as_ref()
            .filter(|c| c.shape == shape_kind)
            .map(|c| c.row_for_normal(inputs.normal));
        ShapePlacementResult {
            accepted: true,
            anchor: inputs.place_pos,
            cells: vec![inputs.place_pos],
            block: row,
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

    /// Furniture consumer: seat the clicker in the free seat nearest the
    /// clicked cell (horizontal distance to the seat's world anchor, so the
    /// pick is facing-correct; declaration order breaks a tie). The claim is
    /// UNCONDITIONAL once the target is
    /// furniture — a fully occupied piece ABSORBS the click (see module docs)
    /// so the initiating client, which cannot see occupancy on its replica,
    /// never mispredicts a placement. One PASS: a sneak click holding a
    /// placeable block defers to the placement consumer (sneak-to-build).
    fn try_sit(&self, pos: [i32; 3], player: PlayerId, actor: &PlayerSnapshot) -> bool {
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
    fn predict_sit(&self, pos: [i32; 3], actor: &PlayerSnapshot) -> bool {
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

/// Resolve the chain family at init: the shared shape kind and its three
/// axis rows (registry-only calls, legal on any instance — the bakes and the
/// placement plan run on both). `None` when the pack content didn't load (a
/// row renamed or removed) — the chair half of the mod keeps working and
/// chains fall back to the row's static (cube) shape.
fn resolve_chains() -> Option<Chains> {
    Some(Chains {
        shape: resolve_shape("furniture:chain")?,
        rows: [
            resolve_block("furniture:chain")?,
            resolve_block("furniture:chain_ns")?,
            resolve_block("furniture:chain_ew")?,
        ],
    })
}

/// Whether the held item places a block (its row carries a `block` link) —
/// the gate the sneak-defer rule reads. Registry-only, legal on any
/// instance; an unresolvable id reads as "not a block".
fn held_places_a_block(held: Option<ItemId>) -> bool {
    let Some(id) = held else {
        return false;
    };
    item_names(vec![id])
        .into_iter()
        .next()
        .flatten()
        .and_then(|name| item_info(&name))
        .is_some_and(|info| info.block.is_some())
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

mod_sdk::register_mod!(Furniture);
