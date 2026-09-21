//! The rules an action a mob performs on the world must meet before anything
//! changes: reach and a clear line from its eye, and what a dig or a
//! construction placement needs of the world. The host call requesting the
//! action and the drain committing it judge through these same functions, so
//! a request that was valid is re-proven, never assumed, at its turn.

use mod_api::ActionRefusal;
use petramond_math::facing::Facing;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::{Aabb, Block};
use petramond_world::construction::{self, Record};
use petramond_world::item::ItemStack;
use petramond_world::world::placement::{self, HeldRotation, PlaceInputs, PlacementPlan};

use crate::player::{Player, RayFilter, RaycastHit};

use super::construction::CellStatus;
use super::World;

/// A live mob resolved for one action.
#[derive(Clone, Copy, Debug)]
pub struct Actor {
    pub index: usize,
    pub id: u64,
    pub eye: WorldPos,
    pub eye_height: f32,
    pub reach: f32,
    /// Where it is looking, head and all. `None` for a body only imagined
    /// somewhere, which may turn as the work asks.
    pub gaze: Option<Vec3>,
}

impl Actor {
    /// The same actor with its feet at `feet` instead, free to look anywhere.
    pub fn standing_at(&self, feet: WorldPos) -> Actor {
        Actor {
            eye: feet + Vec3::new(0.0, self.eye_height, 0.0),
            gaze: None,
            ..*self
        }
    }
}

/// The click behind a placement: the point looked at and the way the
/// placer faces, as the placement rules read a player's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub at: WorldPos,
    pub facing: Facing,
}

/// What the placement rules answered each distinct click of one judgement —
/// the face hit and the way faced. Many aims, from many imagined feet, land
/// on the same face looking the same way, and the rules are asked once.
pub type JudgedClicks = Vec<(
    (IVec3, IVec3, Facing),
    Result<(PlacementPlan, ItemStack), ActionRefusal>,
)>;

/// What a click places: the write the placement rules answer it with, and
/// the one item it lays.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub plan: PlacementPlan,
    pub paid: ItemStack,
    pub click: Click,
}

/// A dig that may proceed: the block and the tool stack (`None` = hands).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DigTarget {
    pub block: Block,
    pub tool: Option<ItemStack>,
}

/// What a construction placement request comes to.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaceCheck {
    Ready(Placement),
    Satisfied,
    Refused(ActionRefusal),
}

impl World {
    /// Resolve `mob_id` as an actor: a live mob.
    pub fn actor(&self, mob_id: u64) -> Result<Actor, ActionRefusal> {
        let index = self
            .mobs()
            .index_of_id(mob_id)
            .ok_or(ActionRefusal::NoActor)?;
        let mob = &self.mobs().instances()[index];
        if mob.is_dead() {
            return Err(ActionRefusal::NoActor);
        }
        let def = crate::mob::def(mob.kind);
        Ok(Actor {
            index,
            id: mob_id,
            eye: mob.pos + Vec3::new(0.0, def.eye_height, 0.0),
            eye_height: def.eye_height,
            reach: def.reach,
            gaze: Some(look_dir(mob.yaw + mob.head_yaw, mob.head_pitch)),
        })
    }

    /// Whether the nearest of `cells` lies within `actor`'s reach.
    fn within_reach(&self, actor: &Actor, cells: &[IVec3]) -> Result<(), ActionRefusal> {
        let nearest = cells
            .iter()
            .map(|&cell| nearest_point(actor, cell).length())
            .min_by(f32::total_cmp)
            .ok_or(ActionRefusal::NothingToDo)?;
        if nearest > actor.reach {
            return Err(ActionRefusal::OutOfReach);
        }
        Ok(())
    }

    /// What a crosshair along `dir` from `actor`'s eye rests on.
    fn crosshair(&self, actor: &Actor, dir: Vec3) -> Option<(RaycastHit, f32)> {
        Player::raycast_filtered(actor.eye, dir, actor.reach, RayFilter::Selectable, self)
    }

    /// The click on the block at `pos` itself (a dig, a use): where `actor`
    /// is looking, or for an imagined body any seen point of the block.
    pub fn block_click(&self, actor: &Actor, pos: IVec3) -> Result<WorldPos, ActionRefusal> {
        self.within_reach(actor, &[pos])?;
        let lands = |dir: Vec3| {
            self.crosshair(actor, dir)
                .filter(|(hit, _)| hit.block == pos)
                .map(|(_, dist)| actor.eye + dir * dist)
        };
        if let Some(gaze) = actor.gaze {
            return lands(gaze).ok_or(ActionRefusal::NotAimed);
        }
        let centre = WorldPos::block_center(pos) - actor.eye;
        let mut aims = Vec::with_capacity(5);
        if let Some((min, max)) = self.selection_box_at(pos.x, pos.y, pos.z) {
            let mid = (Vec3::from(min) + Vec3::from(max)) * 0.5;
            aims.push(WorldPos::block_min(pos) + mid - actor.eye);
        }
        aims.push(centre);
        for axis in 0..3 {
            if centre[axis].abs() > 0.5 {
                let mut face = centre;
                face[axis] -= centre[axis].signum() * 0.45;
                aims.push(face);
            }
        }
        aims.into_iter()
            .filter_map(Vec3::try_normalize)
            .find_map(lands)
            .ok_or(ActionRefusal::NoLineOfSight)
    }

    /// The click that builds toward `want`, the object `record` anchors: a
    /// face beside it (or matter standing in its cell that a placement
    /// replaces) under the crosshair, clicked with one of the `paying` items
    /// in hand, to which the placement rules every click meets answer with a
    /// write the record asks for. The live actor clicks where it is looking;
    /// an imagined one may make any click its eye allows.
    pub fn placement_click(
        &self,
        actor: &Actor,
        record: &Record,
        want: &PlacementPlan,
        paying: &[ItemStack],
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
        judged: &mut JudgedClicks,
    ) -> Result<Placement, ActionRefusal> {
        let mut along = |dir: Vec3| -> Result<Placement, ActionRefusal> {
            let (hit, dist) = self
                .crosshair(actor, dir)
                .ok_or(ActionRefusal::NoLineOfSight)?;
            if hit.normal == IVec3::ZERO {
                return Err(ActionRefusal::NoLineOfSight);
            }
            let looked_at = Block::from_id(self.chunk_block(hit.block.x, hit.block.y, hit.block.z));
            let target = placement::build_position(looked_at, hit.block, hit.normal);
            let builds = want
                .writes
                .iter()
                .any(|w| w.cell == target || (w.cell == hit.block && w.augments));
            if !builds {
                return Err(ActionRefusal::NoLineOfSight);
            }
            let click = Click {
                at: actor.eye + dir * dist,
                facing: crate::server::placement::facing_from_forward(dir),
            };
            let key = (hit.block, hit.normal, click.facing);
            let known = judged
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, a)| a.clone());
            let answer = known.unwrap_or_else(|| {
                let answer = self.click_outcome(record, want, paying, &hit, click.facing, occupied);
                judged.push((key, answer.clone()));
                answer
            });
            answer.map(|(plan, paid)| Placement { plan, paid, click })
        };
        if let Some(gaze) = actor.gaze {
            return along(gaze).map_err(|refusal| match refusal {
                ActionRefusal::NoLineOfSight | ActionRefusal::Misaligned => ActionRefusal::NotAimed,
                other => other,
            });
        }
        // Of the clicks that fail, the one nearest to placing says why.
        let mut worst = ActionRefusal::NoLineOfSight;
        let rank = |refusal: &ActionRefusal| match refusal {
            ActionRefusal::BodyInTheWay => 2,
            ActionRefusal::Misaligned => 1,
            _ => 0,
        };
        for aim in self.click_candidates(actor, want) {
            let Some(dir) = aim.try_normalize() else {
                continue;
            };
            match along(dir) {
                Ok(placement) => return Ok(placement),
                Err(refusal) if rank(&refusal) > rank(&worst) => worst = refusal,
                Err(_) => {}
            }
        }
        Err(worst)
    }

    /// Whether the world holds up what `want` anchors, as its row declares:
    /// named apart from the clicks, because no other click would help.
    fn holds_up(&self, want: &PlacementPlan) -> bool {
        want.writes
            .iter()
            .find(|w| w.cell == want.anchor)
            .is_some_and(|w| self.placement_support_ok(w.block, want.anchor))
    }

    /// What the placement rules make of one click toward `want`, with each
    /// paying item in hand under each turn of the held rotation.
    fn click_outcome(
        &self,
        record: &Record,
        want: &PlacementPlan,
        paying: &[ItemStack],
        hit: &RaycastHit,
        facing: Facing,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Result<(PlacementPlan, ItemStack), ActionRefusal> {
        let Some(anchor) = want.writes.iter().find(|w| w.cell == want.anchor) else {
            return Err(ActionRefusal::NothingToDo);
        };
        // A click the placement rules refuse is the wrong click: another may
        // do. Support the row declares is judged before any click is tried.
        let mut refusal = ActionRefusal::Misaligned;
        for stack in paying {
            let paid = ItemStack { count: 1, ..*stack };
            let block = clicked_row(paid.item, anchor.block);
            for held_rotation in HeldRotation::each(paid.item) {
                let inputs = PlaceInputs::of_click(
                    self,
                    hit.block,
                    hit.normal,
                    facing,
                    held_rotation,
                    Some(paid.item),
                );
                let mut body = false;
                let plan = self.placement_plan(block, &inputs, &mut |cell, boxes| {
                    let hit = occupied(cell, boxes);
                    body |= hit;
                    hit
                });
                match plan {
                    Some(plan) if construction::advances(self, &plan, want, record, &paid) => {
                        return Ok((plan, paid));
                    }
                    None if body => refusal = ActionRefusal::BodyInTheWay,
                    _ => {}
                }
            }
        }
        Err(refusal)
    }

    /// Points, relative to the eye, worth a look when building `writes`: on
    /// each face beside the object that is turned toward the eye.
    fn click_candidates(&self, actor: &Actor, writes: &PlacementPlan) -> Vec<Vec3> {
        const FACES: [IVec3; 6] = [
            IVec3::X,
            IVec3::NEG_X,
            IVec3::Y,
            IVec3::NEG_Y,
            IVec3::Z,
            IVec3::NEG_Z,
        ];
        let mut aims = Vec::new();
        for w in &writes.writes {
            let centre = WorldPos::block_center(w.cell) - actor.eye;
            let own = Block::from_id(self.chunk_block(w.cell.x, w.cell.y, w.cell.z));
            if placement::replaces_in_place(own) {
                aims.push(centre);
            }
            // Parts already standing in the cell are clicked on their own
            // faces: the ones turned toward the eye.
            if w.augments {
                if let Some((min, max)) = self.selection_box_at(w.cell.x, w.cell.y, w.cell.z) {
                    let (min, max) = (Vec3::from(min), Vec3::from(max));
                    let corner = WorldPos::block_min(w.cell) - actor.eye;
                    let mid = corner + (min + max) * 0.5;
                    for face in FACES {
                        let out = face.as_vec3();
                        let on_face = mid + out * ((max - min) * 0.5).dot(out.abs());
                        if on_face.dot(out) < 0.0 {
                            aims.push(on_face);
                        }
                    }
                }
            }
            for face in FACES {
                let n = w.cell + face;
                let out = face.as_vec3();
                let on_face = centre + out * 0.5;
                if writes.writes.iter().any(|o| o.cell == n)
                    || on_face.dot(out) <= 0.0
                    || self.physics_block(n.x, n.y, n.z).is_replaceable()
                {
                    continue;
                }
                // A hair past the face, so the ray crosses it.
                let through = on_face + out * 0.02;
                let (u, v) = (
                    IVec3::new(face.y, face.z, face.x),
                    IVec3::new(face.z, face.x, face.y),
                );
                aims.push(through);
                for (a, b) in [(0.3, 0.0), (-0.3, 0.0), (0.0, 0.3), (0.0, -0.3)] {
                    aims.push(through + u.as_vec3() * a + v.as_vec3() * b);
                }
                if let Some((min, max)) = self.selection_box_at(n.x, n.y, n.z) {
                    let mid = (Vec3::from(min) + Vec3::from(max)) * 0.5;
                    aims.push(WorldPos::block_min(n) + mid - actor.eye);
                }
            }
        }
        aims
    }

    /// Whether `actor` may dig the block at `pos` with the tool in
    /// `tool_slot` of its container.
    pub fn dig_check(
        &self,
        actor: &Actor,
        pos: IVec3,
        tool_slot: Option<u32>,
    ) -> Result<DigTarget, ActionRefusal> {
        if !self.physics_cell_final_at(pos.x, pos.y, pos.z) {
            return Err(ActionRefusal::Unloaded);
        }
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        if block == Block::Air {
            return Err(ActionRefusal::NothingToDo);
        }
        if block.hardness() < 0.0 || block.is_fluid() {
            return Err(ActionRefusal::Unbreakable);
        }
        self.block_click(actor, pos)?;
        let tool = match tool_slot {
            None => None,
            Some(slot) => Some(
                self.mobs().instances()[actor.index]
                    .container()
                    .slots
                    .get(slot as usize)
                    .copied()
                    .flatten()
                    .ok_or(ActionRefusal::NoTool)?,
            ),
        };
        Ok(DigTarget { block, tool })
    }

    /// Where `actor`, standing at each of `feet`, would look to build `record`
    /// at `pos`: the click [`Self::place_check`] would accept from there.
    pub fn placement_aims(
        &self,
        actor: &Actor,
        feet: &[WorldPos],
        pos: IVec3,
        record: &Record,
    ) -> Vec<Result<WorldPos, ActionRefusal>> {
        let refused = |refusal| vec![Err(refusal); feet.len()];
        let (missing, writes) = match self.construction_status(pos, record) {
            CellStatus::Satisfied | CellStatus::Pending(_) => {
                return refused(ActionRefusal::NothingToDo)
            }
            CellStatus::Unloaded => return refused(ActionRefusal::Unloaded),
            CellStatus::Clear { .. } => return refused(ActionRefusal::Obstructed),
            CellStatus::Unsupported(_) => return refused(ActionRefusal::Unsupported),
            CellStatus::Place { missing, writes } => (missing, writes),
        };
        if !construction::has_placement_face(self, &writes) {
            return refused(ActionRefusal::NoFace);
        }
        if !self.holds_up(&writes) {
            return refused(ActionRefusal::NoSupport);
        }
        let cells: Vec<IVec3> = writes.writes.iter().map(|w| w.cell).collect();
        let mut judged = JudgedClicks::new();
        feet.iter()
            .map(|&feet| {
                let standing = actor.standing_at(feet);
                self.within_reach(&standing, &cells)?;
                // Bodies move on before the actor gets there.
                let click = self.placement_click(
                    &standing,
                    record,
                    &writes,
                    &missing,
                    &mut |_, _| false,
                    &mut judged,
                )?;
                Ok(click.click.at)
            })
            .collect()
    }

    /// Whether `actor` may build `record` at `pos` now. `occupied` answers
    /// whether a body stands in given boxes at a cell; `pay` requires the
    /// actor to carry the cost.
    pub fn place_check(
        &self,
        actor: &Actor,
        pos: IVec3,
        record: &Record,
        pay: bool,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlaceCheck {
        let (missing, writes) = match self.construction_status(pos, record) {
            CellStatus::Satisfied => return PlaceCheck::Satisfied,
            CellStatus::Unloaded => return PlaceCheck::Refused(ActionRefusal::Unloaded),
            CellStatus::Clear { .. } => return PlaceCheck::Refused(ActionRefusal::Obstructed),
            CellStatus::Pending(_) => return PlaceCheck::Refused(ActionRefusal::NothingToDo),
            CellStatus::Unsupported(_) => return PlaceCheck::Refused(ActionRefusal::Unsupported),
            CellStatus::Place { missing, writes } => (missing, writes),
        };
        let cells: Vec<IVec3> = writes.writes.iter().map(|w| w.cell).collect();
        if let Err(refusal) = self.within_reach(actor, &cells) {
            return PlaceCheck::Refused(refusal);
        }
        if !construction::has_placement_face(self, &writes) {
            return PlaceCheck::Refused(ActionRefusal::NoFace);
        }
        if !self.holds_up(&writes) {
            return PlaceCheck::Refused(ActionRefusal::NoSupport);
        }
        let placement = match self.placement_click(
            actor,
            record,
            &writes,
            &missing,
            occupied,
            &mut JudgedClicks::new(),
        ) {
            Ok(placement) => placement,
            Err(refusal) => return PlaceCheck::Refused(refusal),
        };
        if pay
            && !self.mobs().instances()[actor.index]
                .container()
                .holds_all(std::slice::from_ref(&placement.paid))
        {
            return PlaceCheck::Refused(ActionRefusal::MissingItems);
        }
        PlaceCheck::Ready(placement)
    }
}

impl World {
    /// Whether a living, non-spectating player (as the roster last published
    /// them) or a live mob stands in `boxes` at `cell`.
    pub fn body_in_the_way(&self, cell: IVec3, boxes: &[Aabb]) -> bool {
        self.player_roster().iter().any(|p| {
            p.health > 0
                && !p.spectator
                && petramond_world::body::Body::new(
                    WorldPos::new(p.pos[0], p.pos[1], p.pos[2]),
                    crate::player::HALF_W,
                    crate::player::HEIGHT,
                )
                .overlaps_block_boxes(cell, boxes)
        }) || self.mobs().any_overlapping_boxes(cell, boxes)
    }
}

/// The row a click with `item` in hand hands the placement rules to build
/// `wanted`: the item's own block, unless `wanted` is a row this item pays for
/// that no click picks for it (a sampled variant, a row its construction rule
/// names) — those are handed over already chosen, as a sampled variant is.
fn clicked_row(item: petramond_world::item::ItemType, wanted: Block) -> Block {
    let Some(base) = item.as_block() else {
        return wanted;
    };
    let picked_by_click = base == wanted
        || base
            .facing_rows()
            .into_iter()
            .flatten()
            .any(|row| *row == wanted)
        || base.flipped_row() == Some(wanted);
    if picked_by_click || construction::paying_item(wanted) != Ok(item) {
        base
    } else {
        wanted
    }
}

/// The point of `cell` nearest `actor`'s eye, relative to the eye: what its
/// reach is measured to.
fn nearest_point(actor: &Actor, cell: IVec3) -> Vec3 {
    let lo = WorldPos::block_min(cell) - actor.eye;
    Vec3::ZERO.clamp(lo, lo + Vec3::ONE)
}

/// The unit direction of a look turned `yaw` and tilted `pitch` (up is
/// positive), in the mobs' `-Z`-forward convention.
fn look_dir(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
}
