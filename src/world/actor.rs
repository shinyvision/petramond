use crate::world::ServerWorld;
use mod_api::ActionRefusal;
use petramond_math::facing::Facing;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::{Aabb, Block};
use petramond_world::construction::{self, Record};
use petramond_world::item::ItemStack;
use petramond_world::world::placement::{
    self, click_spots, HeldRotation, PlaceInputs, PlacementPlan,
};
use petramond_world::world::raycast;

use petramond_world::world::raycast::{RayFilter, RaycastHit};

use super::construction::CellStatus;

#[derive(Clone, Copy, Debug)]
pub struct Actor {
    pub id: u64,
    pub eye: WorldPos,
    pub eye_height: f32,
    pub reach: f32,
    pub gaze: Option<Vec3>,
}

impl Actor {
    pub fn standing_at(&self, feet: WorldPos) -> Actor {
        Actor {
            eye: feet + Vec3::new(0.0, self.eye_height, 0.0),
            gaze: None,
            ..*self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub at: WorldPos,
    pub facing: Facing,
}

pub type JudgedClicks = Vec<(
    (IVec3, IVec3, Facing),
    Result<(PlacementPlan, ItemStack), ActionRefusal>,
)>;

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub plan: PlacementPlan,
    pub paid: ItemStack,
    pub click: Click,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DigTarget {
    pub block: Block,
    pub tool: Option<ItemStack>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaceCheck {
    Ready(Placement),
    Satisfied,
    Refused(ActionRefusal),
}

impl ServerWorld {
    pub fn actor(&self, mob_id: u64) -> Result<Actor, ActionRefusal> {
        let mob = self.mobs().live(mob_id).ok_or(ActionRefusal::NoActor)?;
        let def = crate::mob::def(mob.kind);
        Ok(Actor {
            id: mob_id,
            eye: mob.pos + Vec3::new(0.0, def.eye_height, 0.0),
            eye_height: def.eye_height,
            reach: def.reach,
            gaze: Some(look_dir(mob.yaw + mob.head_yaw, mob.head_pitch)),
        })
    }

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

    fn crosshair(&self, actor: &Actor, dir: Vec3) -> Option<(RaycastHit, f32)> {
        raycast::filtered(
            actor.eye,
            dir,
            actor.reach,
            RayFilter::Selectable,
            &self.data,
        )
    }

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
        if let Some((min, max)) = self.data.selection_box_at(pos.x, pos.y, pos.z) {
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
            let looked_at =
                Block::from_id(self.data.chunk_block(hit.block.x, hit.block.y, hit.block.z));
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
                facing: petramond_math::facing::Facing::toward_viewer(dir),
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

    fn holds_up(&self, want: &PlacementPlan) -> bool {
        want.writes
            .iter()
            .find(|w| w.cell == want.anchor)
            .is_some_and(|w| self.data.placement_support_ok(w.block, want.anchor))
    }

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
        let mut refusal = ActionRefusal::Misaligned;
        for spot in click_spots(hit.spot.to_array(), hit.normal) {
            for stack in paying {
                let paid = ItemStack { count: 1, ..*stack };
                let block = clicked_row(paid.item, anchor.block);
                for held_rotation in HeldRotation::each(paid.item) {
                    let inputs = PlaceInputs::of_click(
                        self.data(),
                        hit.block,
                        hit.normal,
                        spot,
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
                        Some(plan)
                            if construction::advances(&self.data, &plan, want, record, &paid) =>
                        {
                            return Ok((plan, paid));
                        }
                        None if body => refusal = ActionRefusal::BodyInTheWay,
                        _ => {}
                    }
                }
            }
        }
        Err(refusal)
    }

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
            let own = Block::from_id(self.data.chunk_block(w.cell.x, w.cell.y, w.cell.z));
            if placement::replaces_in_place(own) {
                aims.push(centre);
            }
            if w.augments {
                if let Some((min, max)) = self.data.selection_box_at(w.cell.x, w.cell.y, w.cell.z) {
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
                    || self.data.physics_block(n.x, n.y, n.z).is_replaceable()
                {
                    continue;
                }
                let through = on_face + out * 0.02;
                let (u, v) = (
                    IVec3::new(face.y, face.z, face.x),
                    IVec3::new(face.z, face.x, face.y),
                );
                aims.push(through);
                for (a, b) in [(0.3, 0.0), (-0.3, 0.0), (0.0, 0.3), (0.0, -0.3)] {
                    aims.push(through + u.as_vec3() * a + v.as_vec3() * b);
                }
                if let Some((min, max)) = self.data.selection_box_at(n.x, n.y, n.z) {
                    let mid = (Vec3::from(min) + Vec3::from(max)) * 0.5;
                    aims.push(WorldPos::block_min(n) + mid - actor.eye);
                }
            }
        }
        aims
    }

    pub fn dig_check(
        &self,
        actor: &Actor,
        pos: IVec3,
        tool_slot: Option<u32>,
    ) -> Result<DigTarget, ActionRefusal> {
        if !self.physics_cell_final_at(pos.x, pos.y, pos.z) {
            return Err(ActionRefusal::Unloaded);
        }
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
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
                self.mobs()
                    .get(actor.id)
                    .ok_or(ActionRefusal::NoActor)?
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
        if !construction::has_placement_face(&self.data, &writes) {
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
        if !construction::has_placement_face(&self.data, &writes) {
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
            && self.mobs().get(actor.id).is_none_or(|mob| {
                !mob.container()
                    .holds_all(std::slice::from_ref(&placement.paid))
            })
        {
            return PlaceCheck::Refused(ActionRefusal::MissingItems);
        }
        PlaceCheck::Ready(placement)
    }
}

impl ServerWorld {
    pub fn body_in_the_way(&self, cell: IVec3, boxes: &[Aabb]) -> bool {
        self.player_roster().iter().any(|p| {
            p.health > 0
                && !p.spectator
                && petramond_world::body::Body::new(
                    WorldPos::new(p.pos[0], p.pos[1], p.pos[2]),
                    crate::world::session::PLAYER_HALF_W,
                    crate::world::session::PLAYER_HEIGHT,
                )
                .overlaps_block_boxes(cell, boxes)
        }) || self.mobs().any_overlapping_boxes(cell, boxes)
    }
}

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

fn nearest_point(actor: &Actor, cell: IVec3) -> Vec3 {
    let lo = WorldPos::block_min(cell) - actor.eye;
    Vec3::ZERO.clamp(lo, lo + Vec3::ONE)
}

fn look_dir(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
}
