//! The two shape facet traits — the composable seam a shape family implements
//! so consumers dispatch through it instead of matching a closed enum.
//!
//! The split is by AUDIENCE (see the plan / WIKI/modding.md): [`ShapeSim`] is
//! authoritative and deterministic (collision, support, nav — the multiplayer
//! tick reads it, on the server and re-evaluated against the client replica);
//! [`ShapeRender`] is client presentation (selection outline, item form). Both
//! read the world ONLY through the primitive [`ShapeNeighborhood`] seam — the
//! sim `World` implements it (server and client replica alike), and so does
//! the mesher's padded section snapshot, so ONE family implementation resolves
//! identically wherever it runs and cannot reach past the seam. A family unit
//! struct implements one or both; the [`ShapeKindDef`](super::ShapeKindDef)
//! row binds the singletons.

use crate::atlas::Tile;
use crate::block_model::BlockModelKind;
use crate::mathh::IVec3;

use super::super::{Aabb, Block, CellPart, ShapeBox};
use super::neighborhood::{ShapeNeighborhood, ShapeState};
use super::ShapeParams;

// The placement facet lives with the placement types (world::placement) but is
// bound per family here, so re-export it on the facet path.
pub(crate) use crate::world::placement::ShapePlacement;

/// Everything a family needs to resolve ONE cell's geometry: the cell, its
/// row parameters, the primitive world seam, and the per-tile world tint
/// (the biome grass/water channel) it should bake into its face styles.
///
/// A family reads the world ONLY through `nb`, which is what lets the same
/// implementation serve the sim thread and the off-thread chunk mesher.
pub struct ShapeCtx<'a> {
    pub nb: &'a dyn ShapeNeighborhood,
    pub pos: crate::mathh::IVec3,
    pub block: Block,
    pub params: &'a ShapeParams,
    /// The world tint for a tile — biome-driven, so it belongs to the caller,
    /// not the family. A cell's `petramond:tint` is NOT applied here: that
    /// rides the shared `ShapeBox::apply_tint` post-pass so no family can
    /// forget it.
    pub tint_for: &'a dyn Fn(crate::atlas::Tile) -> [f32; 3],
    /// This cell's stored tint for one of its [`CellPart`]s, if any — supplied
    /// by the caller because it comes from cell KV, which is not on the shape
    /// seam. A family does NOT use this to colour its boxes (the post-pass
    /// owns the multiply); it is here so a family that has SEVERAL parts can
    /// answer questions only it can, like whether its parts agree closely
    /// enough to mesh as one cube. Answers `None` on the geometry-only probe
    /// paths (out-of-cell AO / occupancy), which never look at colour.
    pub part_tint: &'a dyn Fn(CellPart) -> Option<[f32; 3]>,
}

/// The `part_tint` a geometry-only [`ShapeCtx`] carries: no cell is coloured
/// on the AO / occupancy probe paths, and reaching a neighbour section's KV
/// through the mesh pad is not possible anyway.
pub const NO_PART_TINT: &dyn Fn(CellPart) -> Option<[f32; 3]> = &|_| None;

/// The world a shape sees when there IS no world: air everywhere, no state.
/// Resolves a shape's position-LESS answers — the light twin of
/// [`ShapeSim::default_boxes`], used to bake the per-block-id default aperture
/// table at load. A stateful family reads its own default state through it (a
/// stair answers its default facing), which is exactly the fallback a cell
/// with no stored state should get.
pub struct NoNeighborhood;

impl ShapeNeighborhood for NoNeighborhood {
    fn block(&self, _pos: IVec3) -> Block {
        Block::Air
    }
    fn shape_state(&self, _pos: IVec3) -> ShapeState {
        ShapeState::NONE
    }
}

/// The half-cell octant `(ix, iy, iz)` as a cell-local AABB — the quantization
/// unit shared by sub-cell occupancy and the light apertures, so "does my
/// matter fill this octant" has ONE meaning.
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
/// both halves are load-bearing, and both were learned from playtest:
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

/// The aperture quadrant bit of octant `(ix, iy, iz)` on the face whose normal
/// runs along `axis`.
#[inline]
fn aperture_bit(axis: usize, ix: usize, iy: usize, iz: usize) -> u8 {
    let (a, b) = match axis {
        0 => (iz, iy),
        1 => (ix, iz),
        _ => (ix, iy),
    };
    1u8 << (b * 2 + a)
}

/// The union of a box list as a selection box: `None` for an empty list (there
/// is nothing to outline) and for an exact full unit cube (the default
/// selection already IS the cell). The one place box lists collapse to an
/// outline, shared by every family that outlines what it collides with.
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

/// A family's answer to [`ShapeSim::full_face`]: the face is complete, and it
/// is (or is not) the face of a full cube. Material rules (opaque-only joins,
/// the pane opt-out tag, a mount's opacity requirement) bind CUBE faces only —
/// a shaped face that is geometrically complete joins by geometry alone.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FullFace {
    Cube,
    Shaped,
}

/// Whether the shape at `pos` LIES FLAT on its cell floor — its matter fills
/// every bottom octant, so the cell rests on the ground the way a cover or a
/// plate does rather than rooting in it like a plant. Asked of the shape, so a
/// mod block shaped like a cover behaves like one with no engine edit.
pub fn rests_flat_on_floor(nb: &dyn ShapeNeighborhood, pos: IVec3, block: Block) -> bool {
    let k = block.shape_kind_def();
    (0..2).all(|ix| {
        (0..2).all(|iz| {
            let (lo, hi) = octant_box(ix, 0, iz);
            k.sim.occupies_pocket(&k.params, nb, pos, block, lo, hi)
        })
    })
}

/// Ask the family at `q` whether its face with outward normal `dir` is
/// complete — the one dispatch every cross-family asker uses.
pub fn full_face_at(nb: &dyn ShapeNeighborhood, q: IVec3, dir: IVec3) -> Option<FullFace> {
    let b = nb.block(q);
    let k = b.shape_kind_def();
    k.sim.full_face(&k.params, nb, q, b, dir)
}

/// Packed per-face light apertures: six 4-bit quadrant masks, face order
/// `-x,+x,-y,+y,-z,+z` (bits `face*4 .. face*4+4`). `0` = the face blocks
/// light entirely, `0b1111` = fully open. The ONE light-geometry currency the
/// families produce and the flood consumes.
pub const LIGHT_APERTURES_OPEN: u32 = 0x00FF_FFFF;

/// Pack a per-direction quadrant-mask function into the aperture word (see
/// [`LIGHT_APERTURES_OPEN`] for the layout).
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

/// The 4-bit quadrant mask for the face with outward normal `dir`, out of a
/// packed aperture word. The flood inlines this bit read against its fused
/// cell word; the shape/light tests are the remaining callers.
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

/// Sim-side shape behavior: authoritative, deterministic. A headless server
/// calls every method here and never touches [`ShapeRender`].
pub trait ShapeSim: Send + Sync + 'static {
    /// The block's position-aware collision boxes — the resolve behind
    /// [`World::collision_boxes_at`]. The default is the row's position-less
    /// [`Block::collision_boxes`] (right for cube/cross/crop/torch/lowered);
    /// stateful and neighbour-aware families override. Reads the world ONLY
    /// through the primitive seam (the sim `World` implements it), so one
    /// implementation resolves identically wherever it is asked.
    fn collision_boxes(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        block: Block,
    ) -> &'static [Aabb] {
        block.collision_boxes()
    }

    /// The row's position-LESS box set: what this shape collides as with no
    /// world context (the item form, a fresh placement gate, the fallback a
    /// cell with no resolvable state uses). The default is the row's authored
    /// `collision` boxes; a family whose canonical form is derived (a stair's
    /// default facing, a bare no-neighbour fence post) overrides.
    fn default_boxes(&self, _params: &ShapeParams, block: Block) -> &'static [Aabb] {
        block.row_collision()
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

    /// The sub-cell [`CellPart`]s this cell is made of, each with the block
    /// that part is made OF — the cross-family "what is in this cell" question
    /// the couriers ask so they can address per-part data (a layer's tint) and
    /// drop each part as its own material.
    ///
    /// `None` — the default, and the answer of every family but the stacking
    /// slab — means the cell is ONE whole part of its own block: the couriers
    /// use the bare KV keys and the drop is the row's ordinary drop spec.
    /// `Some(list)` means the cell is COMPOSED, and each part drops the item
    /// of its own block instead. The numbering must match the family's
    /// [`ShapeBox::part`]s and its placement writes (see [`CellPart`]).
    fn parts(
        &self,
        _params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _block: Block,
    ) -> Option<Vec<(CellPart, Block)>> {
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

    /// How this family participates in light propagation (called only for a
    /// non-opaque row — the caller short-circuits opaque full cubes). `Open`
    /// is the default; a family whose passage depends on per-cell state
    /// answers [`Shaped`](crate::block::BlockLightShape::Shaped) and supplies
    /// the apertures through [`light_apertures`](Self::light_apertures).
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

    /// The cell's packed per-face light apertures — see
    /// [`pack_light_apertures`] for the layout. Read for every `Shaped` cell,
    /// so the flood consults family-answered bits and holds no family
    /// knowledge. Deterministic (the server's light bake is authoritative).
    ///
    /// The default DERIVES them from [`occupies_pocket`](Self::occupies_pocket):
    /// a face quadrant is open exactly when nothing sits against the boundary
    /// in the middle of it (see [`aperture_probe`]). No family should override
    /// this — "what
    /// blocks light" is "what matter is there", and a second hand-written
    /// answer is a second thing to keep in sync (stairs and slabs each had
    /// one, and the stair's silently used its PLACED shape, not its refined
    /// one).
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

    /// Whether navigation reads a cell of this shape as solid even though its
    /// real collision boxes are not a full cube — true only for the fence
    /// family (a lone fence must be a wall or no pen holds). Everything else is
    /// classified from its boxes, so the default is `false`.
    fn nav_reads_solid(&self, _params: &ShapeParams) -> bool {
        false
    }
}

/// Render-side shape behavior: client-only presentation, no determinism
/// requirement. Never called on a headless server.
pub trait ShapeRender: Send + Sync + 'static {
    /// The selection / raycast-target box (union) — the resolve behind
    /// [`World::selection_box_at`]. `None` = the full-cube default. Must agree
    /// with [`ShapeSim::collision_boxes`] so "aim inside the outline" hits the
    /// real box — which is why it reads through the same primitive seam.
    /// Default is the family's position-less
    /// [`default_selection_box`](Self::default_selection_box).
    fn selection_box(
        &self,
        params: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        self.default_selection_box(params, block)
    }

    /// The position-LESS selection box: what this shape targets and outlines
    /// with no world context — the render twin of
    /// [`ShapeSim::default_boxes`], and the resolve behind
    /// [`Block::visual_aabb`]. `None` = an ordinary full cube (which needs no
    /// outline of its own) or a cell with nothing to aim at.
    ///
    /// The default is the union of the row's collision boxes, so a shape that
    /// COLLIDES with what it draws needs nothing here. A shape whose visible
    /// form is deliberately not its collision — a walk-through thin cover, a
    /// no-collision bbmodel — overrides, which is what keeps it aimable.
    fn default_selection_box(
        &self,
        _params: &ShapeParams,
        block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        union_box(block.collision_boxes())
    }

    /// Resolve this cell's drawn box set — THE box producer for a box-shaped
    /// family. The chunk mesher, the neighbour-occupancy cull, and any other
    /// consumer of a shape's real form call this and nothing else, so a family
    /// is written once instead of once per consumer.
    ///
    /// Default: no boxes. A family whose form is not a box set (full cube,
    /// plants, torch, bbmodel) leaves it alone and draws through its own path.
    fn boxes(&self, _ctx: &ShapeCtx<'_>, _out: &mut Vec<ShapeBox>) {}

    /// Whether THIS cell's resolved form is exactly the material's full cube,
    /// so it should mesh through the cube fast path (greedy merge included)
    /// instead of the box emitter. A uniform full slab stack is the only
    /// engine case; the merge is load-bearing for streaming perf.
    fn meshes_as_cube(&self, _ctx: &ShapeCtx<'_>) -> bool {
        false
    }

    /// Whether a targeting ray picks this shape against its RESOLVED COLLISION
    /// BOXES rather than stopping on a single box. True for every family whose
    /// real form is a box set (stair, slab, pane, fence, a Layer-3 bake): the
    /// aimed geometry is then the collided geometry by construction, so aiming
    /// through a gap misses and aiming at a part hits, with no per-family ray
    /// code. A family whose form is not a box set keeps this `false` — a full
    /// cube stops the ray on cell entry, and the torch / bbmodel families run
    /// their own precise tests (a tilted pole, an alpha-tested surface).
    fn picks_by_boxes(&self, _params: &ShapeParams) -> bool {
        false
    }

    /// The item KIND + geometry decision for a block of this shape — the
    /// per-shape arm folded out of `ItemType::render_kind`. Drives the inventory
    /// icon, the dropped entity, and the in-hand form identically. The default
    /// is a plain cube icon.
    fn item_render(&self, _params: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Cube(block)
    }
}

/// What an item form looks like — the shape decides once, for icon / dropped /
/// in-hand. Folds the per-shape half of `ItemType::render_kind` together with
/// the true-geometry choice `render::item_cube` re-derives. `ItemType` resolves
/// [`ItemSprite`](Self::ItemSprite) against the item's own row (its `sprite`
/// field), the one piece that is item data rather than shape data.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemRender {
    /// Use the ITEM's own sprite (row `sprite`, fallback art) — the thin /
    /// connection shapes whose item art is flat (torch, pane, door, ladder).
    ItemSprite,
    /// A specific atlas tile as a flat sprite (a plant's top tile).
    Tile(Tile),
    /// A plain full-cube icon (the plain cube).
    Cube(Block),
    /// True baked geometry built from the family + the held state (stair, slab,
    /// fence) — the same helpers the chunk mesher uses.
    Geometry(Block),
    /// A baked bbmodel, everywhere.
    Model(BlockModelKind),
}
