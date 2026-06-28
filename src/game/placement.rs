use super::{tick::TickEvents, Game};
use crate::block::{Block, BlockInteraction, RenderShape};
use crate::furnace::Facing;
use crate::mathh::{IVec3, Vec3};
use crate::torch::TorchPlacement;

impl Game {
    /// Placement / interaction, on the tick: consume a buffered secondary-button press
    /// once. Right-clicking a placed interactable block uses its block-owned capability
    /// rather than placing into the cell — unless sneaking, which falls through so the
    /// player can still build against it.
    pub(super) fn tick_place(&mut self, events: &mut TickEvents) {
        if !std::mem::take(&mut self.pending_place) {
            return;
        }
        let interacted = !self.intent_sneak && self.try_open_interactable();
        if !interacted {
            // Capture the held block before `try_place` consumes it: on success that is
            // exactly the block placed, which the client maps to a place sound.
            let held = self
                .player
                .inventory
                .selected()
                .and_then(|s| s.item.as_block());
            if self.try_place() {
                events.placed_block = held;
            }
        }
    }

    /// If the look target has a secondary-use capability, apply it and return
    /// `true` (consuming the right-click).
    fn try_open_interactable(&mut self) -> bool {
        let Some(h) = self.look else { return false };
        let block = Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
        match block.interaction() {
            BlockInteraction::OpenCraftingTable => {
                self.request_open_table = true;
                true
            }
            BlockInteraction::OpenFurnace => {
                self.request_open_furnace = Some(h.block);
                true
            }
            BlockInteraction::OpenChest => {
                self.request_open_chest = Some(h.block);
                true
            }
            BlockInteraction::OpenFurnitureWorkbench => {
                self.request_open_workbench = Some(h.block);
                true
            }
            // Right-clicking a door toggles it: the open/closed bit flips on this tick
            // (so collision updates at once and the player can step through), and the
            // visual swing is eased from the door's current angle. Seed the swing entry
            // BEFORE the toggle so it starts from the old pose, then eases to the new one.
            BlockInteraction::ToggleDoor => {
                if let Some(lower) = self.world.door_lower_cell(h.block.x, h.block.y, h.block.z) {
                    let start = self.door_swing_angle(lower);
                    self.door_swings.entry(lower).or_insert(start);
                    self.world.toggle_door(h.block);
                    // Flick the hand for the interaction (same jab as opening a chest).
                    self.toggled_door = true;
                }
                true
            }
            BlockInteraction::None => false,
        }
    }

    pub(super) fn try_place(&mut self) -> bool {
        let Some(h) = self.look else { return false };
        if h.normal == IVec3::ZERO {
            return false;
        }

        let block = match self.player.inventory.selected() {
            Some(stack) => match stack.item.as_block() {
                Some(b) if b != Block::Air => b,
                _ => return false,
            },
            None => return false,
        };

        // Right-clicking a replaceable block (short grass, a fern…) while holding a block
        // places straight INTO its cell, overwriting it with no drop — the block just
        // disappears, as if the cell were empty. Otherwise the placement builds against
        // the clicked face. Air is replaceable too (a placement may overwrite it) but is
        // never itself a raycast hit, so exclude it. `p` then feeds the torch support
        // gate, the model footprint, and the final replaceable check uniformly.
        let looked_at = Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
        let replacing_in_place = looked_at.is_replaceable() && looked_at != Block::Air;
        let p = if replacing_in_place {
            h.block
        } else {
            h.block + h.normal
        };

        // A torch only mounts on a floor or wall (never a ceiling) and needs a full solid
        // face to attach to. Resolve that up front so an invalid spot is a no-op (the
        // click neither places nor consumes the torch) rather than leaving a floating one.
        // When REPLACING a plant the torch always drops to the FLOOR of that cell — so
        // right-clicking grass from any angle, even its side, stands a floor torch where
        // the grass was instead of failing on the side face's would-be wall mount.
        let torch_placement = if block == Block::Torch {
            let tp = if replacing_in_place {
                TorchPlacement::Floor
            } else {
                match TorchPlacement::from_place_normal(h.normal) {
                    Some(tp) => tp,
                    None => return false,
                }
            };
            let s = tp.support_cell(p);
            if !Block::from_id(self.world.chunk_block(s.x, s.y, s.z)).is_opaque() {
                return false;
            }
            Some(tp)
        } else {
            None
        };

        // A bbmodel block places its WHOLE footprint (the workbench is 2×2×1): every
        // occupied cell must be loaded + replaceable AND clear of the player/mobs, or the
        // placement fails as a unit (nothing placed, the held item kept). Multi-cell
        // models, and models marked directionalView, are oriented from the player's
        // facing; `p` is the front-left bottom anchor from the player's view.
        if let RenderShape::Model(kind) = block.render_shape() {
            let player_facing = facing_from_forward(self.cam.forward());
            let multi_cell = crate::block_model::instance(kind).cells.len() > 1;
            let facing = if block.directional_view() || multi_cell {
                player_facing
            } else {
                crate::block_model::DEFAULT_MODEL_FACING
            };
            let base = if block.directional_view() || multi_cell {
                crate::block_model::base_from_front_left_anchor(p, kind, facing)
            } else {
                p
            };
            if !self.world.model_footprint_clear_facing(base, kind, facing) {
                return false;
            }
            let blocked = crate::block_model::oriented_footprint_cells(base, kind, facing)
                .into_iter()
                .any(|(c, off)| {
                    self.player.intersects_block(c)
                        || self.world.mobs().any_overlapping_boxes(
                            c,
                            crate::block_model::collision_boxes_oriented(kind, off, facing),
                        )
                });
            if !blocked && self.world.place_model_block_facing(base, block, facing) {
                self.player.inventory.decrement_selected();
                return true;
            }
            return false;
        }

        // A door is a 2-tall thin block: its lower cell is `p`, the upper is the cell
        // above. Both must be loaded + replaceable AND give it a floor to stand on
        // (`door_footprint_clear`), and the closed slab must not trap the player or a
        // mob. It sits on the edge nearest the placer (the player's facing). Placement
        // + the paired door state live in `World::place_door`.
        if block.render_shape() == RenderShape::Door {
            let facing = facing_from_forward(self.cam.forward());
            let upper = p + IVec3::new(0, 1, 0);
            if !self.world.door_footprint_clear(p) {
                return false;
            }
            let closed = |top: bool| {
                crate::door::collision_boxes(crate::door::DoorState {
                    facing,
                    open: false,
                    top,
                })
            };
            let blocked = [(p, false), (upper, true)].into_iter().any(|(c, top)| {
                self.player.intersects_block(c)
                    || self.world.mobs().any_overlapping_boxes(c, closed(top))
            });
            if !blocked && self.world.place_door(p, block, facing) {
                self.player.inventory.decrement_selected();
                return true;
            }
            return false;
        }

        // Substrate gate: a block that roots in a particular ground — a flower in soil, a
        // cactus in sand, a mushroom on soil or stone — places only when the cell directly
        // below is a ground it accepts (`can_root_on`). Blocks with no such rule (almost
        // all of them) accept anything; a torch is gated by its own opaque-face check
        // above. Staying put once placed is the separate job of the FRAGILE behaviour.
        let below = Block::from_id(self.world.chunk_block(p.x, p.y - 1, p.z));
        if !block.can_root_on(below) {
            return false;
        }

        let target = Block::from_id(self.world.chunk_block(p.x, p.y, p.z));
        // A block with no collision box (a torch, grass, a fern, …) traps nothing, so it
        // may be placed inside an entity; a block that WOULD collide can't be placed where
        // it overlaps the player or a mob — the placement simply fails (the click does
        // nothing and the held item isn't consumed).
        let collides = block.blocks_movement();
        let clear_of_player = !collides || !self.player.intersects_block(p);
        let clear_of_mobs = !collides || !self.world.mobs().any_overlapping_placement(p, block);
        if target.is_replaceable()
            && clear_of_player
            && clear_of_mobs
            && self.world.set_block_world(p.x, p.y, p.z, block)
        {
            // A placed furnace/chest gets an empty block-entity from the moment it
            // exists. Blocks marked directionalView have their front oriented to face
            // the player; a torch records how it is mounted (floor vs which wall) for
            // the mesher + outline.
            let placed_facing = if block.directional_view() {
                facing_from_forward(self.cam.forward())
            } else {
                crate::block_model::DEFAULT_MODEL_FACING
            };
            if block == Block::Furnace {
                self.world.insert_furnace(p, placed_facing);
            } else if block == Block::Chest {
                self.world.insert_chest(p, placed_facing);
            } else if let Some(tp) = torch_placement {
                self.world.insert_torch(p, tp);
            }
            self.player.inventory.decrement_selected();
            true
        } else {
            false
        }
    }
}

/// The furnace facing for a block placed while looking along `forward`: the front
/// (mouth) points back toward the player — opposite the camera's horizontal look
/// direction — snapped to the nearest cardinal.
pub(super) fn facing_from_forward(forward: Vec3) -> Facing {
    let (fx, fz) = (-forward.x, -forward.z);
    if fx.abs() >= fz.abs() {
        if fx >= 0.0 {
            Facing::East
        } else {
            Facing::West
        }
    } else if fz >= 0.0 {
        Facing::South
    } else {
        Facing::North
    }
}
