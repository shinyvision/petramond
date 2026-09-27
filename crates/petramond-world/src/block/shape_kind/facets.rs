use crate::block_model::BlockModelKind;
use crate::mathh::IVec3;
use crate::tile::Tile;

use super::super::{Aabb, Block, CellPart, ShapeBox};
use super::neighborhood::{ShapeNeighborhood, ShapeState};
use super::ShapeParams;

pub use crate::world::placement::ShapePlacement;

pub struct ShapeCtx<'a> {
    pub nb: &'a dyn ShapeNeighborhood,
    pub pos: crate::mathh::IVec3,
    pub block: Block,
    pub params: &'a ShapeParams,
    pub tint_for: &'a dyn Fn(crate::tile::Tile) -> [f32; 3],
    pub part_tint: &'a dyn Fn(CellPart) -> Option<[f32; 3]>,
}

pub const NO_PART_TINT: &dyn Fn(CellPart) -> Option<[f32; 3]> = &|_| None;

pub struct NoNeighborhood;

impl ShapeNeighborhood for NoNeighborhood {
    fn block(&self, _pos: IVec3) -> Block {
        Block::Air
    }
    fn shape_state(&self, _pos: IVec3) -> ShapeState {
        ShapeState::NONE
    }
}

#[inline]
pub fn octant_box(ix: usize, iy: usize, iz: usize) -> ([f32; 3], [f32; 3]) {
    let lo = [ix as f32 * 0.5, iy as f32 * 0.5, iz as f32 * 0.5];
    (
        [lo[0], lo[1], lo[2]],
        [lo[0] + 0.5, lo[1] + 0.5, lo[2] + 0.5],
    )
}

/// Half-extent of a light aperture probe: HALF A TEXEL, so a box authored on
/// the 1/16 grid is either hit squarely or missed cleanly.
const APERTURE_PROBE: f32 = 1.0 / 32.0;

/// What a light aperture probes for quadrant `(ix, iy, iz)`: a small box at
/// the CENTRE of that quadrant, pressed against the cell boundary its `axis`
/// face lies on.
///
/// Centre, not the whole quadrant, and boundary, not the volume behind it —
/// both halves determine whether a boundary seals:
/// - probing the whole octant VOLUME reads farmland (15/16 tall) as sealing
///   its own top, which floods its cell black and drags every neighbouring
///   face's smooth light down with it;
/// - probing the whole quadrant's boundary RECT reads a cactus as sealed on
///   all four sides, because its cap plates clip the extreme texel of each
///   quadrant edge — the same black cell, for the same reason.
///
/// A quadrant is coarse (a quarter of a face); asking "is there matter in the
/// middle of it, right against the boundary" is the answer that matches what
/// the shape looks like from outside.
fn aperture_probe(axis: usize, ix: usize, iy: usize, iz: usize) -> ([f32; 3], [f32; 3]) {
    let (qlo, qhi) = octant_box(ix, iy, iz);
    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    for a in 0..3 {
        let centre = (qlo[a] + qhi[a]) * 0.5;
        lo[a] = centre - APERTURE_PROBE;
        hi[a] = centre + APERTURE_PROBE;
    }
    if [ix, iy, iz][axis] == 0 {
        lo[axis] = 0.0;
        hi[axis] = APERTURE_PROBE;
    } else {
        lo[axis] = 1.0 - APERTURE_PROBE;
        hi[axis] = 1.0;
    }
    (lo, hi)
}

#[inline]
fn aperture_bit(axis: usize, ix: usize, iy: usize, iz: usize) -> u8 {
    let (a, b) = match axis {
        0 => (iz, iy),
        1 => (ix, iz),
        _ => (ix, iy),
    };
    1u8 << (b * 2 + a)
}

pub fn union_box(boxes: &[Aabb]) -> Option<([f32; 3], [f32; 3])> {
    if boxes.is_empty() {
        return None;
    }
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for b in boxes {
        for i in 0..3 {
            mn[i] = mn[i].min(b.min[i]);
            mx[i] = mx[i].max(b.max[i]);
        }
    }
    (mn != [0.0; 3] || mx != [1.0; 3]).then_some((mn, mx))
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FullFace {
    Cube,
    Shaped,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeshEmitter {
    Cube,
    Boxes,
    Plant(PlantPlanes),
    Pole,
    Model,
    Nothing,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlantPlanes {
    Cross,
    Crop,
}

#[derive(Copy, Clone, Debug)]
pub struct RowFacts {
    pub flags: crate::block::BlockFlags,
    pub corners: bool,
    pub authored_collision: bool,
}

pub fn rests_flat_on_floor(nb: &dyn ShapeNeighborhood, pos: IVec3, block: Block) -> bool {
    let k = block.shape_kind_def();
    (0..2).all(|ix| {
        (0..2).all(|iz| {
            let (lo, hi) = octant_box(ix, 0, iz);
            k.sim.occupies_pocket(&k.params, nb, pos, block, lo, hi)
        })
    })
}

/// Whether the shape at `pos` presents a COMPLETE face of matter toward `dir`
/// — every quadrant of that boundary occupied.
///
/// The geometric twin of [`rests_flat_on_floor`], and the same probe the light
/// apertures use ([`aperture_probe`]): half a texel at the CENTRE of each
/// quadrant, pressed against the boundary. It is what an arbitrary pile of
/// matter — a box set, a pack's baked shape — can answer about itself without
/// anyone maintaining a list, so a furniture counter's full-width worktop holds
/// a lamp while a bottom slab's mid-cell top does not, and neither family is
/// named anywhere.
///
/// It reads [`ShapeSim::occupies_pocket`], so it is only meaningful for the
/// families that ANSWER that — the arbitrary-matter ones, which call this from
/// their own [`ShapeSim::full_face`]. It is NOT a general "is this face solid"
/// oracle: a plain cube leaves `occupies_pocket` at its `false` default and
/// would read as empty here. Ask [`full_face_at`] instead, which dispatches to
/// whichever answer the family actually owns.
pub fn face_is_solid(nb: &dyn ShapeNeighborhood, pos: IVec3, dir: IVec3) -> bool {
    let block = nb.block(pos);
    let k = block.shape_kind_def();
    let d = [dir.x, dir.y, dir.z];
    let Some(axis) = d.iter().position(|&c| c != 0) else {
        return false;
    };
    let layer = usize::from(d[axis] > 0);
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    (0..2).all(|i| {
        (0..2).all(|j| {
            let mut q = [0usize; 3];
            q[axis] = layer;
            q[u] = i;
            q[v] = j;
            let (lo, hi) = aperture_probe(axis, q[0], q[1], q[2]);
            k.sim.occupies_pocket(&k.params, nb, pos, block, lo, hi)
        })
    })
}

pub fn full_face_at(nb: &dyn ShapeNeighborhood, q: IVec3, dir: IVec3) -> Option<FullFace> {
    let b = nb.block(q);
    let k = b.shape_kind_def();
    k.sim.full_face(&k.params, nb, q, b, dir)
}

pub const LIGHT_APERTURES_OPEN: u32 = 0x00FF_FFFF;

pub fn pack_light_apertures(mut mask: impl FnMut((i32, i32, i32)) -> u8) -> u32 {
    const DIRS: [(i32, i32, i32); 6] = [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ];
    let mut out = 0u32;
    for (i, &d) in DIRS.iter().enumerate() {
        out |= u32::from(mask(d) & 0xF) << (i * 4);
    }
    out
}

#[allow(dead_code)]
#[inline]
pub fn light_aperture_face(masks: u32, dir: (i32, i32, i32)) -> u8 {
    let i = match dir {
        (-1, 0, 0) => 0,
        (1, 0, 0) => 1,
        (0, -1, 0) => 2,
        (0, 1, 0) => 3,
        (0, 0, -1) => 4,
        _ => 5,
    };
    ((masks >> (i * 4)) & 0xF) as u8
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ShapeMount {
    pub cell: IVec3,
    pub normal: IVec3,
}

pub trait ShapeSim: Send + Sync + 'static {
    fn rotate_y(&self, block: Block, state: ShapeState) -> crate::block::rotation::CellRotation {
        crate::block::rotation::common(block, state)
    }

    fn collision_boxes(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        block: Block,
    ) -> &'static [Aabb] {
        block.collision_boxes()
    }

    fn default_boxes(&self, _params: &ShapeParams, block: Block) -> &'static [Aabb] {
        block.row_collision()
    }

    fn target_boxes(
        &self,
        params: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
        out: &mut Vec<crate::block::PosedBox>,
    ) {
        out.extend(
            self.collision_boxes(params, nb, pos, block)
                .iter()
                .map(|&aabb| crate::block::PosedBox { aabb, pose: None }),
        );
    }

    /// The cell this shape GRIPS and the outward normal of the face it grips,
    /// for a family whose support comes from its PLACEMENT rather than the
    /// row's declarative `support_dir`. `None` (the default) = the
    /// declarative rule governs, which is every ordinary block.
    ///
    /// A wall torch grips the wall behind it; a wall panel grips the wall it
    /// is mounted on. Answering here is what keeps the fragility rule free of
    /// family names, so a pack's wall lantern or rope gets the same support
    /// treatment with no engine edit.
    fn mount(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _block: Block,
    ) -> Option<ShapeMount> {
        None
    }

    /// Whether the face of this cell with outward normal `dir` is a COMPLETE
    /// surface — and whether it is the face of a full CUBE (material rules
    /// bind cube faces only; a partial shape's complete face joins/supports by
    /// geometry alone). THE cross-family geometry question: connection rules
    /// ask it of their neighbours (a fence arm joins a stair's flat back),
    /// mounts ask it of their support (a wall torch, a ladder wall) — the
    /// asker never knows which family answers. Default: no complete face
    /// (plants, thin shapes, unresolved bakes).
    fn full_face(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _block: Block,
        _dir: IVec3,
    ) -> Option<FullFace> {
        None
    }

    /// Which [`CellPart`]s this cell is made of, each with the block that part is made of.
    /// Couriers use it for per-part data like a layer's tint, and to drop each part separately.
    ///
    /// `None` (every family except the stacking slab) means one whole part: couriers use the
    /// bare KV keys and the drop is the row's usual spec. `Some(list)` means a composed cell,
    /// and each part drops its own block's item. Numbering has to match the family's
    /// [`ShapeBox::part`]s and its placement writes.
    fn parts(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _block: Block,
    ) -> Option<Vec<(CellPart, Block)>> {
        None
    }

    fn keeping_parts(
        &self,
        _params: &ShapeParams,
        _block: Block,
        _state: ShapeState,
        _keep: &[CellPart],
    ) -> Option<(Block, ShapeState)> {
        None
    }

    /// Re-resolve this cell's STORED state from its neighbourhood — the
    /// neighbour-refinement resolver. The engine runs it at EDIT time (a
    /// block update reaches the cell), byte-compares the result with what is
    /// stored, and on a change writes it and updates the cell's own
    /// neighbours in turn (`World::refine_shape_states_around`), so reading a
    /// refined shape is always a free stored-state decode — never a
    /// neighbourhood walk. Must be a PURE function of the neighbourhood
    /// (deterministic; prediction re-runs it against the replica). The
    /// default keeps the state as-is (a stateless or placement-only-state
    /// family); a family whose shape depends on neighbours overrides. A
    /// WASM-resolved shape keeps the default here — its resolver is the
    /// guest bake, driven by the same edit fan-out through the bake pump.
    fn refine_state(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _block: Block,
        state: ShapeState,
    ) -> ShapeState {
        state
    }

    fn light_shape(&self, _params: &ShapeParams, _block: Block) -> crate::block::BlockLightShape {
        crate::block::BlockLightShape::Open
    }

    /// Whether this cell's matter overlaps the cell-local pocket `(lo, hi)` —
    /// the sub-cell occupancy oracle. It is pure GEOMETRY (no tiles, no
    /// presentation), which is why it lives on the sim side: the mesher's AO
    /// probes and the light flood's apertures are the same question asked by
    /// two consumers, and answering it twice is how they drift.
    ///
    /// A family answers at whatever granularity its shape warrants, and the
    /// granularity is DELIBERATE, not laziness: stairs and slabs answer by
    /// half-cell OCTANT, because AO is quantized to 0..3, because the light
    /// apertures are quantized to quadrants anyway, and because the two
    /// meshers must agree byte for byte.
    fn occupies_pocket(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _block: Block,
        _lo: [f32; 3],
        _hi: [f32; 3],
    ) -> bool {
        false
    }

    fn shades_pocket(
        &self,
        params: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        self.occupies_pocket(params, nb, pos, block, lo, hi)
    }

    fn light_apertures(
        &self,
        params: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
    ) -> u32 {
        pack_light_apertures(|dir| {
            let d = [dir.0, dir.1, dir.2];
            let Some(axis) = d.iter().position(|&c| c != 0) else {
                return 0;
            };
            let layer = usize::from(d[axis] > 0);
            let mut open = 0u8;
            for iy in 0..2 {
                for iz in 0..2 {
                    for ix in 0..2 {
                        if [ix, iy, iz][axis] != layer {
                            continue;
                        }
                        let (lo, hi) = aperture_probe(axis, ix, iy, iz);
                        if !self.occupies_pocket(params, nb, pos, block, lo, hi) {
                            open |= aperture_bit(axis, ix, iy, iz);
                        }
                    }
                }
            }
            open
        })
    }

    fn collision_state_free(&self) -> bool {
        false
    }

    fn refines(&self, _params: &ShapeParams) -> bool {
        false
    }

    fn nav_follows_row(&self) -> bool {
        false
    }

    fn compound_members(
        &self,
        _params: &ShapeParams,
        _block: Block,
        _pos: IVec3,
        _state: ShapeState,
    ) -> Option<Vec<(IVec3, ShapeState)>> {
        None
    }

    fn accepts_row_uv_rotation(&self) -> bool {
        false
    }

    fn hosts_fluid(&self) -> bool {
        false
    }

    fn faces_by_row(&self) -> bool {
        false
    }

    fn row_flags(&self) -> crate::block::BlockFlags {
        crate::block::BlockFlags::NONE
    }

    fn validate_row(&self, _params: &ShapeParams, _row: &RowFacts) -> Result<(), String> {
        Ok(())
    }

    fn nav_reads_solid(&self, _params: &ShapeParams) -> bool {
        false
    }
}

pub trait ShapeRender: Send + Sync + 'static {
    fn selection_box(
        &self,
        params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        self.default_selection_box(params, block)
    }

    fn default_selection_box(
        &self,
        _params: &ShapeParams,
        block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        union_box(block.collision_boxes())
    }

    fn boxes(&self, _ctx: &ShapeCtx<'_>, _out: &mut Vec<ShapeBox>) {}

    fn meshes_as_cube(&self, _ctx: &ShapeCtx<'_>) -> bool {
        false
    }

    /// Whether a targeting ray picks this shape against its resolved TARGET
    /// BOXES ([`ShapeSim::target_boxes`] — the collision boxes unless the
    /// family says otherwise) rather than stopping on a single box. True for
    /// every family whose real form is a box set (stair, slab, pane, fence, a
    /// static box set, a WASM shape bake): the aimed geometry is then the
    /// resolved geometry by construction, so aiming through a gap misses and
    /// aiming at a part (posed or not) hits, with no per-family ray code. A
    /// family whose form is not a box set keeps this `false` — a full cube
    /// stops the ray on cell entry, and the torch / bbmodel families run
    /// their own precise tests (a tilted pole, an alpha-tested surface).
    fn picks_by_boxes(&self, _params: &ShapeParams) -> bool {
        false
    }

    fn precise_pick(&self, params: &ShapeParams) -> bool {
        self.picks_by_boxes(params)
    }

    fn mesh_emitter(&self, _params: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Cube
    }

    fn animated_pose(
        &self,
        _params: &ShapeParams,
        block: Block,
        state: ShapeState,
    ) -> Option<crate::animated_model::AnimatedPose> {
        use super::neighborhood::CellView;
        let facing = if block.directional_view() {
            crate::block_state::EntityFront::from_cell(state).0
        } else {
            crate::facing::Facing::default()
        };
        Some(crate::animated_model::AnimatedPose {
            facing,
            variant: 0,
            open: false,
        })
    }

    /// The cell-local boxes this shape's ITEM draws, when its item is true
    /// geometry. Empty (the default) = the item is a sprite, a plain cube, or
    /// a bbmodel, drawn by its own path.
    ///
    /// The family answers because an item is NOT always the placed form: a
    /// fence item is an authored two-post SEGMENT where the placed cell with
    /// no neighbours resolves to a bare post. `state` is the HELD state (a
    /// stair's facing/half, a slab's layers), so the icon shows what a click
    /// would place.
    fn item_boxes(
        &self,
        _params: &ShapeParams,
        _block: Block,
        _state: crate::block_state::HeldBlockState,
        _out: &mut Vec<crate::block::ItemBox>,
    ) {
    }

    fn item_render(&self, _params: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::BlockForm(block)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemRender {
    ItemSprite,
    Tile(Tile),
    BlockForm(Block),
    Model(BlockModelKind),
}
