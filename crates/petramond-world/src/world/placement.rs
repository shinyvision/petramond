use super::data::WorldData;
use crate::block::SupportDir;
use crate::block::{Aabb, Block, CellPart, ShapeState};
use crate::block_state::{HeldBlockState, LogAxis, StairHalf};
use crate::facing::Facing;
use crate::item::ItemType;
use crate::mathh::IVec3;
use crate::slab::{SlabRotation, SlabSlot};

pub mod authored;

pub fn replaces_in_place(looked_at: Block) -> bool {
    looked_at.is_replaceable() && looked_at != Block::Air
}

pub fn build_position(looked_at: Block, hit: IVec3, normal: IVec3) -> IVec3 {
    if replaces_in_place(looked_at) {
        hit
    } else {
        hit + normal
    }
}

fn rotation_count(block: crate::block::Block) -> u8 {
    if block.is_axial() {
        2
    } else {
        block.shape_kind_def().placement.held_rotations()
    }
}

impl HeldRotation {
    pub fn each(item: ItemType) -> impl Iterator<Item = HeldRotation> {
        let count = match item.as_block() {
            Some(block) if rotatable_block(block) => rotation_count(block),
            _ => 1,
        };
        (0..count).map(move |rotation| HeldRotation {
            item: (rotation != 0).then_some(item),
            rotation,
        })
    }
}

fn rotatable_block(block: crate::block::Block) -> bool {
    rotation_count(block) > 1
}

pub fn click_spots(spot: [f32; 3], normal: IVec3) -> impl Iterator<Item = [f32; 3]> {
    let count = if normal.y == 0 { 3 } else { 1 };
    [spot, [spot[0], 0.25, spot[2]], [spot[0], 0.75, spot[2]]]
        .into_iter()
        .take(count)
}

#[derive(Clone, Debug, Default)]

pub struct HeldRotation {
    pub item: Option<ItemType>,
    pub rotation: u8,
}

impl HeldRotation {
    pub fn toggle(&mut self, selected: Option<ItemType>) {
        let Some(item) = selected else {
            self.clear();
            return;
        };
        if !item.as_block().is_some_and(rotatable_block) {
            self.clear();
            return;
        }
        if self.item == Some(item) {
            let count = item.as_block().map_or(1, rotation_count).max(1);
            self.rotation = (self.rotation + 1) % count;
        } else {
            self.item = Some(item);
            self.rotation = 1 % item.as_block().map_or(1, rotation_count).max(1);
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.item = None;
        self.rotation = 0;
    }

    pub fn apply_wire(&mut self, counter: u8, selected: Option<ItemType>) {
        if counter == self.rotation {
            return;
        }
        if counter == 0 {
            self.clear();
        } else {
            self.rotation = counter;
            self.item = selected;
        }
    }

    #[inline]
    fn active(&self, selected: Option<ItemType>) -> bool {
        let Some(item) = selected else {
            return false;
        };
        self.item == Some(item)
            && self.rotation != 0
            && item.as_block().is_some_and(rotatable_block)
    }

    #[inline]
    pub fn held_block_state(&self, selected: Option<ItemType>) -> HeldBlockState {
        let Some(block) = selected.and_then(ItemType::as_block) else {
            return HeldBlockState::None;
        };
        if let Some(held) = block
            .shape_kind_def()
            .placement
            .held_state(block, self, selected)
        {
            return held;
        }
        if block.is_axial() {
            return HeldBlockState::Log(if self.active(selected) {
                LogAxis::X
            } else {
                LogAxis::Y
            });
        }
        HeldBlockState::None
    }

    #[inline]
    pub fn stair_half(&self, selected: Option<ItemType>) -> StairHalf {
        if self.active(selected) {
            StairHalf::Top
        } else {
            StairHalf::Bottom
        }
    }

    #[inline]
    pub fn slab_rotation(&self, selected: Option<ItemType>) -> crate::slab::SlabRotation {
        if self.active(selected) {
            crate::slab::SlabRotation::from_index(self.rotation)
        } else {
            crate::slab::SlabRotation::Bottom
        }
    }

    #[inline]
    pub fn log_axis_for_facing(
        &self,
        selected: Option<ItemType>,
        facing: crate::facing::Facing,
    ) -> LogAxis {
        if !self.active(selected) {
            return LogAxis::Y;
        }
        match facing {
            crate::facing::Facing::East | crate::facing::Facing::West => LogAxis::X,
            crate::facing::Facing::North | crate::facing::Facing::South => LogAxis::Z,
        }
    }
}

pub struct PlaceInputs {
    pub hit: IVec3,
    pub normal: IVec3,
    pub spot: [f32; 3],
    pub place_pos: IVec3,
    pub replacing_in_place: bool,
    pub player_facing: Facing,
    pub held_rotation: HeldRotation,
    pub held: Option<crate::item::ItemType>,
}

impl PlaceInputs {
    #[allow(clippy::too_many_arguments)]
    pub fn of_click(
        w: &WorldData,
        hit: IVec3,
        normal: IVec3,
        spot: [f32; 3],
        player_facing: Facing,
        held_rotation: HeldRotation,
        held: Option<ItemType>,
    ) -> Self {
        let looked_at = Block::from_id(w.chunk_block(hit.x, hit.y, hit.z));
        Self {
            hit,
            normal,
            spot,
            place_pos: build_position(looked_at, hit, normal),
            replacing_in_place: replaces_in_place(looked_at),
            player_facing,
            held_rotation,
            held,
        }
    }

    #[inline]
    pub fn support_side(&self) -> Option<Facing> {
        Facing::from_horizontal_normal(-self.normal)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CellWrite {
    pub cell: IVec3,
    pub block: Block,
    pub state: ShapeState,
    pub part: CellPart,
    /// Whether this write AUGMENTS the cell (adds a part to one that already
    /// holds others) rather than replacing it whole.
    ///
    /// A block write clears the cell's whole KV map, which is exactly right
    /// when the cell is being replaced — air holds no data — and wrong when a
    /// second slab stacks into a dyed one, where it would silently eat the
    /// sibling layer's colour. An augmenting write carries the map across;
    /// the courier then overwrites only `part`'s own keys.
    pub augments: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacementPlan {
    pub anchor: IVec3,
    pub writes: Vec<CellWrite>,
}

impl PlacementPlan {
    pub fn single(cell: IVec3, block: Block, state: ShapeState) -> Self {
        Self {
            anchor: cell,
            writes: vec![Self::whole(cell, block, state)],
        }
    }

    pub fn single_part(
        cell: IVec3,
        block: Block,
        state: ShapeState,
        part: CellPart,
        augments: bool,
    ) -> Self {
        Self {
            anchor: cell,
            writes: vec![CellWrite {
                cell,
                block,
                state,
                part,
                augments,
            }],
        }
    }

    pub fn whole(cell: IVec3, block: Block, state: ShapeState) -> CellWrite {
        CellWrite {
            cell,
            block,
            state,
            part: 0,
            augments: false,
        }
    }

    pub fn cells(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.writes.iter().map(|w| w.cell)
    }

    pub fn anchor_part(&self) -> CellPart {
        self.writes
            .iter()
            .find(|w| w.cell == self.anchor)
            .map_or(0, |w| w.part)
    }
}

pub enum ConstructionWrites {
    Anchor(PlacementPlan),
    Member(IVec3),
}

pub enum PlacementOutcome {
    Refused,
    General,
    Plan(PlacementPlan),
}

pub trait ShapePlacement: Send + Sync + 'static {
    fn held_rotations(&self) -> u8 {
        1
    }

    fn held_state(
        &self,
        _block: Block,
        _rotation: &HeldRotation,
        _selected: Option<ItemType>,
    ) -> Option<HeldBlockState> {
        None
    }

    fn authored_state(&self, _block: Block, state: ShapeState) -> ShapeState {
        state
    }

    fn construction_writes(
        &self,
        block: Block,
        state: ShapeState,
        pos: IVec3,
    ) -> ConstructionWrites {
        ConstructionWrites::Anchor(PlacementPlan::single(
            pos,
            block,
            self.authored_state(block, state),
        ))
    }

    fn member_state(&self, _block: Block, state: ShapeState, _pos: IVec3) -> ShapeState {
        state
    }

    fn authored_plan(
        &self,
        block: Block,
        inputs: &mut authored::Inputs<'_>,
    ) -> Result<PlacementPlan, String> {
        inputs.general(block)
    }

    fn placement_plan(
        &self,
        _w: &WorldData,
        _block: Block,
        _inputs: &PlaceInputs,
        _occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        PlacementOutcome::General
    }
}

/// Validates an accepted custom-shape placement plan. The server and the client's place ghost
/// both call this, so their writes match by construction instead of two hand-kept copies.
/// Refuses a plan that writes past the anchor cell, an anchor more than Chebyshev 2 from
/// `place_pos`, or a `block` override that isn't a sibling row of the same shape kind. A kind
/// belongs to one pack, so a plan can't reach across packs.
/// Returns the anchor and the row to write, the held row by default.
pub fn validate_custom_plan(
    result: &mod_api::ShapePlacementResult,
    held: Block,
    shape_kind: u16,
    place_pos: IVec3,
) -> Option<(IVec3, Block)> {
    let write_block = match result.block {
        None => held,
        Some(b) => {
            let b = Block::from_id(b.0);
            if b.shape_kind().0 != shape_kind {
                return None;
            }
            b
        }
    };
    let anchor = IVec3::new(result.anchor[0], result.anchor[1], result.anchor[2]);
    let single_cell = result.cells.is_empty()
        || (result.cells.len() == 1 && result.cells[0] == anchor.to_array());
    if !single_cell {
        return None;
    }
    let (dx, dy, dz) = (
        (anchor.x - place_pos.x).abs(),
        (anchor.y - place_pos.y).abs(),
        (anchor.z - place_pos.z).abs(),
    );
    if dx.max(dy).max(dz) > 2 {
        return None;
    }
    Some((anchor, write_block))
}

impl WorldData {
    pub fn fragile_supported(&self, pos: IVec3, block: Block) -> bool {
        // A shape whose support comes from its PLACEMENT answers where it
        // grips; the face-completeness test is then the shared one. No family
        // is named here, so a pack's wall lantern supports itself for free.
        let k = block.shape_kind_def();
        if let Some(m) = k.sim.mount(&k.params, self, pos, block) {
            return self.mount_face_complete(m.cell, m.normal);
        }
        let dir = block.support_dir();
        let s = dir.support_cell(pos);
        match dir {
            SupportDir::Below => {
                let ground = self.physics_block(s.x, s.y, s.z);
                if !block.can_root_on(ground) {
                    return false;
                }
                // DECLARED beats derived. A row that stated what its floor must
                // look like keeps that same rule once placed, so the gate that
                // let it be placed and the rule that keeps it there cannot
                // disagree. It has to come first: `rests_flat_on_floor` probes
                // octant VOLUMES, so anything with a foot on the floor — a
                // lantern's 8-wide base — reads as lying flat and would take
                // the cover rule instead of its own.
                if block.roots_face() != crate::block::RootsFace::Any {
                    return self.roots_face_ok(block, crate::mathh::IVec3::Y, s, ground);
                }
                if crate::block::rests_flat_on_floor(self, pos, block) {
                    return super::query::full_unit_cube(self.collision_boxes_at(s.x, s.y, s.z));
                }
                ground.is_opaque()
            }
            SupportDir::Above => {
                super::query::full_unit_cube(self.collision_boxes_at(s.x, s.y, s.z))
                    || Block::from_id(self.chunk_block(s.x, s.y, s.z)).support_dir()
                        == SupportDir::Above
            }
            _ => self.mount_face_complete(s, pos - s),
        }
    }

    pub fn roots_face_ok(&self, block: Block, normal: IVec3, s: IVec3, ground: Block) -> bool {
        match block.roots_face() {
            crate::block::RootsFace::Any => true,
            crate::block::RootsFace::FullCube => {
                ground.is_opaque()
                    && crate::block::full_face_at(self, s, normal)
                        == Some(crate::block::FullFace::Cube)
            }
            crate::block::RootsFace::SolidFace => self.mount_face_complete(s, normal),
        }
    }

    pub fn slab_stack_slot_in_hit(
        &self,
        block: Block,
        hit: IVec3,
        rotation: SlabRotation,
        normal: IVec3,
        player_facing: Facing,
    ) -> Option<SlabSlot> {
        if !crate::slab::is_slab(block) {
            return None;
        }
        let looked_at = Block::from_id(self.chunk_block(hit.x, hit.y, hit.z));
        if !crate::slab::is_slab(looked_at) {
            return None;
        }
        let slot = crate::slab::stack_slot(rotation, normal, player_facing)?;
        crate::slab::can_add_layer(self.slab_state_at(hit.x, hit.y, hit.z), slot).then_some(slot)
    }
    pub fn finish_single_cell_placement(
        &self,
        block: Block,
        p: IVec3,
        state: ShapeState,
        boxes: &[Aabb],
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        if !self.placement_support_ok(block, p) {
            return None;
        }
        let target = Block::from_id(self.chunk_block(p.x, p.y, p.z));
        if !target.is_replaceable() || target == block {
            return None;
        }
        if occupied(p, boxes) {
            return None;
        }
        Some(PlacementPlan::single(p, block, state))
    }
    pub fn placement_support_ok(&self, block: Block, p: IVec3) -> bool {
        let s = block.support_dir().support_cell(p);
        let ground = self.physics_block(s.x, s.y, s.z);
        if !block.can_root_on(ground) {
            return false;
        }
        if !self.roots_face_ok(block, p - s, s, ground) {
            return false;
        }
        // A fragile row supported by a wall or ceiling has no substrate to gate on, since
        // `roots_on` only names grounds. The two rules above would accept open air, and the
        // fragile block update would then shatter the block on dispatch and eat the item.
        // Gating on the fragile rule itself keeps placement and survival in agreement.
        !(block.is_fragile()
            && block.support_dir() != crate::block::SupportDir::Below
            && !self.fragile_supported(p, block))
    }
}
