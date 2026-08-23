//! Mod-driven per-block DRAW SETS: retained presentation geometry a mod
//! submits for a placed block, redrawn every frame and costing no re-mesh.
//!
//! This is the seam under anything a mod SIMULATES rather than stages. A block
//! row swap or a parts mask picks between pictures that were authored in
//! advance; a draw set IS a picture, computed by the mod this tick — liquid
//! running down a channel, a level rising, a needle moving. Because nothing
//! here touches chunk geometry, submitting a new set every tick is the
//! intended use, where a parts change at that rate would re-mesh a section
//! twenty times a second.
//!
//! Lifecycle is the same as every other per-block record: the set dies when
//! the cell's block changes, and it replicates as a whole-set delta (they are
//! a handful of prims, and a partial set is never meaningful).

use std::sync::Arc;

use mod_api::DrawPrim;

use petramond_math::math::IVec3;

use super::store::World;

/// A submitted prim list, SHARED. One block's set is stored once and every
/// consumer after that — the section payload, the per-tick delta, and one copy
/// per recipient session — takes a refcount bump.
///
/// It is a newtype rather than a bare `Arc<[DrawPrim]>` because it crosses the
/// wire, where the shape is an ordinary sequence (serde's `Arc` impls are
/// behind a feature the workspace does not take, and the local connection must
/// keep the refcount rather than round-trip). Same trade as
/// [`SectionBytes`](crate::net::protocol::SectionBytes): in-process is free,
/// TCP pays one pass.
///
/// WHY it is not a `Vec`: a delta lane is filtered PER RECIPIENT, so the lane
/// itself cannot be shared — only its elements can. With a `Vec` inside, a
/// machine redrawing every tick cost every viewer a fresh allocation per prim
/// name, every tick, and that is multiplied by the player count.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawPrims(pub Arc<[DrawPrim]>);

impl DrawPrims {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[DrawPrim] {
        &self.0
    }
}

impl From<Vec<DrawPrim>> for DrawPrims {
    fn from(prims: Vec<DrawPrim>) -> DrawPrims {
        DrawPrims(Arc::from(prims.into_boxed_slice()))
    }
}

impl serde::Serialize for DrawPrims {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for DrawPrims {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<DrawPrims, D::Error> {
        Ok(DrawPrims::from(Vec::<DrawPrim>::deserialize(d)?))
    }
}

/// One resolved prim, ready for the renderer: names have become ids, so the
/// per-frame bake never touches a registry.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockDrawPrim {
    Cuboid {
        min: [f32; 3],
        max: [f32; 3],
        tile: petramond_world::tile::Tile,
        tint: [u8; 3],
        emissive: bool,
    },
    Item {
        at: [f32; 3],
        scale: f32,
        yaw: f32,
        pitch: f32,
        item: petramond_world::item::ItemType,
        tint: [u8; 3],
    },
}

impl BlockDrawPrim {
    /// Resolve an ABI prim's names. `None` = a name this world does not know,
    /// which drops that prim rather than the whole set: a pack referencing one
    /// missing tile should lose one box, not its whole machine.
    pub fn resolve(prim: &DrawPrim) -> Option<BlockDrawPrim> {
        match prim {
            DrawPrim::Cuboid {
                min,
                max,
                tile,
                tint,
                emissive,
            } => {
                let tile = petramond_world::tile::Tile::from_name(tile)?;
                // Degenerate, inverted, or non-finite: a box with an infinite
                // corner passes an ordering test and reaches the vertex buffer.
                let sane =
                    (0..3).all(|a| max[a] > min[a] && min[a].is_finite() && max[a].is_finite());
                sane.then_some(BlockDrawPrim::Cuboid {
                    min: *min,
                    max: *max,
                    tile,
                    tint: *tint,
                    emissive: *emissive,
                })
            }
            DrawPrim::Item {
                at,
                scale,
                yaw,
                pitch,
                item,
                tint,
            } => {
                let item = petramond_world::item::ItemType::by_name(item)?;
                let sane = *scale > 0.0
                    && scale.is_finite()
                    && yaw.is_finite()
                    && pitch.is_finite()
                    && at.iter().all(|v| v.is_finite());
                sane.then_some(BlockDrawPrim::Item {
                    at: *at,
                    scale: *scale,
                    yaw: *yaw,
                    pitch: *pitch,
                    item,
                    tint: *tint,
                })
            }
        }
    }
}

/// One block's retained draw set: the prims AS SUBMITTED (what the wire
/// ships — names, like every other replicated identity) beside the resolved
/// form the renderer bakes, so the per-frame path never touches a registry.
#[derive(Debug, Default, PartialEq)]
pub struct BlockDrawSet {
    pub wire: DrawPrims,
    pub resolved: Vec<BlockDrawPrim>,
    /// Everything above, bounded, in prim space — computed once here so the
    /// per-frame gather can cull a set against the view without walking its
    /// prims. Empty sets bound to nothing and are culled by that.
    pub bounds: Option<DrawBox>,
}

impl BlockDrawSet {
    pub fn new(wire: DrawPrims) -> BlockDrawSet {
        let resolved: Vec<BlockDrawPrim> = wire
            .as_slice()
            .iter()
            .filter_map(BlockDrawPrim::resolve)
            .collect();
        let bounds = prim_bounds(&resolved);
        BlockDrawSet {
            wire,
            resolved,
            bounds,
        }
    }
}

/// A prim-space box in world axes: the bounds of its transformed corners.
/// The transform is a yaw plus a translation, so that is exact rather than
/// conservative.
pub fn world_bounds(to_world: &petramond_math::math::Mat4, lo: [f32; 3], hi: [f32; 3]) -> DrawBox {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for cx in [lo[0], hi[0]] {
        for cy in [lo[1], hi[1]] {
            for cz in [lo[2], hi[2]] {
                let p = to_world.transform_point3(petramond_math::math::Vec3::new(cx, cy, cz));
                for a in 0..3 {
                    mn[a] = mn[a].min(p[a]);
                    mx[a] = mx[a].max(p[a]);
                }
            }
        }
    }
    (mn, mx)
}

/// The prim-space AABB of a whole set. An item prim is a cube of side `scale`
/// about its point AT ANY rotation, so the bound holds however it is turned.
fn prim_bounds(prims: &[BlockDrawPrim]) -> Option<DrawBox> {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    let mut any = false;
    let mut grow = |lo: [f32; 3], hi: [f32; 3]| {
        for a in 0..3 {
            mn[a] = mn[a].min(lo[a]);
            mx[a] = mx[a].max(hi[a]);
        }
    };
    for prim in prims {
        any = true;
        match prim {
            BlockDrawPrim::Cuboid { min, max, .. } => grow(*min, *max),
            BlockDrawPrim::Item { at, scale, .. } => {
                // The diagonal, so a spin about any axis stays inside.
                let r = scale * 0.87;
                grow(
                    [at[0] - r, at[1] - r, at[2] - r],
                    [at[0] + r, at[1] + r, at[2] + r],
                );
            }
        }
    }
    any.then_some((mn, mx))
}

pub type BlockDraw = Arc<BlockDrawSet>;

/// An axis-aligned `(min, max)` box, in whichever space the holder documents.
pub type DrawBox = ([f32; 3], [f32; 3]);

/// A stored set together with its PLACEMENT: the block-space→world transform
/// and the world box that transform puts its prims in.
///
/// Both are resolved when the set is stored (and re-resolved by every path
/// that writes the anchor cell — see
/// [`refresh_block_draw_placement`](World::refresh_block_draw_placement)),
/// because the alternative is what this replaced: the per-frame gather built a
/// transform out of three chunk lookups and transformed eight corners for
/// EVERY loaded set before it could ask whether the set was on screen — cost
/// proportional to what is loaded, which is the one thing a presentation
/// gather may never be.
pub(in crate::world) struct PlacedDraw {
    pub(in crate::world) set: BlockDraw,
    pub(in crate::world) to_world: petramond_math::math::Mat4,
    /// [`BlockDrawSet::bounds`] in world axes; `None` for a set that resolved
    /// to no prims and can therefore never be visible.
    pub(in crate::world) world: Option<DrawBox>,
}

/// One placed block's draw set to draw this frame, with the light at its cell.
///
/// This ONE row type travels from the gather to the renderer's bake unchanged —
/// presentation, scene and renderer each hold a `Vec` of it. It used to be
/// spelled three times, and each re-spelling cost a struct copy and an atomic
/// refcount pair per visible set per frame for nothing: the layers disagreed
/// about the type's name, not about its contents.
#[derive(Clone, Debug)]
pub struct BlockDrawInstance {
    pub pos: IVec3,
    /// Shared with the world — a frame copies a refcount, never the prims.
    pub set: BlockDraw,
    /// Prim space → world. For a model block this is its footprint space
    /// turned by the placed facing, so a mod's geometry follows the model it
    /// was authored against instead of the world axes.
    pub transform: petramond_math::math::Mat4,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

/// The draw sets anchored inside ONE section, plus their union bound.
///
/// The index exists because every other operation on this store is
/// section-shaped — a section payload carries its own sets, an evicted section
/// drops them, a frame gathers the visible ones — and each of those used to
/// walk the WHOLE map. The union bound then makes the frame gather reject a
/// section's worth of sets on one test.
#[derive(Default)]
pub(in crate::world) struct SectionDraws {
    cells: Vec<IVec3>,
    /// Union of the members' world boxes. GROWN on insert and recomputed
    /// exactly on removal: a machine redrawing itself inside its authored
    /// envelope must not pay an O(members) refold every tick, and a bound that
    /// is too big only costs a section that could have been rejected.
    lo: [f32; 3],
    hi: [f32; 3],
}

impl SectionDraws {
    fn empty() -> SectionDraws {
        SectionDraws {
            cells: Vec::new(),
            lo: [f32::MAX; 3],
            hi: [f32::MIN; 3],
        }
    }

    fn grow(&mut self, world: Option<DrawBox>) {
        let Some((lo, hi)) = world else { return };
        for a in 0..3 {
            self.lo[a] = self.lo[a].min(lo[a]);
            self.hi[a] = self.hi[a].max(hi[a]);
        }
    }
}

impl World {
    /// Replace the draw set at `pos`. An empty set clears it.
    ///
    /// It answers nothing on purpose: "is anything stored now" is a question
    /// no caller has, and the ABI's `bool` is about whether the SUBMISSION was
    /// accepted — a distinction that read backwards for a clear, which is a
    /// perfectly successful call that stores nothing.
    pub fn set_block_draw(&mut self, pos: IVec3, prims: DrawPrims) {
        // Keyed at the group ANCHOR, exactly like the block's container and its
        // parts mask: a mod addresses a multi-cell machine by whichever cell it
        // has to hand, and a set stored under a non-anchor cell would never be
        // hit-tested and never forgotten on break.
        let pos = self.container_anchor(pos);
        // Resubmitting the same picture is the COMMON case — a machine at rest
        // redraws itself every tick — so it is answered by comparing the
        // submitted form, before any name resolves or allocation.
        if self
            .draw_stream
            .block_draws
            .get(&pos)
            .is_some_and(|s| s.set.wire == prims)
        {
            return;
        }
        self.store_draw(pos, prims);
    }

    /// The one place a set is stored; an empty submission takes the record
    /// with it.
    fn store_draw(&mut self, pos: IVec3, wire: DrawPrims) {
        if wire.is_empty() {
            if self.remove_draw(pos) {
                self.log_block_draw(pos);
            }
            return;
        }
        self.insert_draw(pos, Arc::new(BlockDrawSet::new(wire)));
        self.log_block_draw(pos);
    }

    /// The block-space→world transform and the world box a set's prims land in
    /// under it — resolved once, here, for every path that stores a set.
    fn draw_placement(
        &self,
        pos: IVec3,
        set: &BlockDrawSet,
    ) -> (petramond_math::math::Mat4, Option<DrawBox>) {
        let to_world = self.block_local_transform(pos);
        let world = set.bounds.map(|(lo, hi)| world_bounds(&to_world, lo, hi));
        (to_world, world)
    }

    /// Store `set` at `pos`, keeping the per-section index and its bound.
    fn insert_draw(&mut self, pos: IVec3, set: BlockDraw) {
        // Outside the world's vertical range there is no section to index it
        // under, and an unindexed record is one nothing can gather or forget.
        let Some(sp) = petramond_world::chunk::SectionPos::from_world(pos.x, pos.y, pos.z) else {
            return;
        };
        let (to_world, world) = self.draw_placement(pos, &set);
        let fresh = self
            .draw_stream
            .block_draws
            .insert(
                pos,
                PlacedDraw {
                    set,
                    to_world,
                    world,
                },
            )
            .is_none();
        let entry = self
            .draw_stream
            .block_draw_sections
            .entry(sp)
            .or_insert_with(SectionDraws::empty);
        if fresh {
            entry.cells.push(pos);
        }
        entry.grow(world);
    }

    /// Drop the set at `pos`, refolding its section's bound exactly (a removal
    /// is rare, and a grow-only bound would otherwise never shrink).
    fn remove_draw(&mut self, pos: IVec3) -> bool {
        if self.draw_stream.block_draws.remove(&pos).is_none() {
            return false;
        }
        let Some(sp) = petramond_world::chunk::SectionPos::from_world(pos.x, pos.y, pos.z) else {
            return true;
        };
        let Some(mut entry) = self.draw_stream.block_draw_sections.remove(&sp) else {
            return true;
        };
        entry.cells.retain(|c| *c != pos);
        if entry.cells.is_empty() {
            return true;
        }
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for c in &entry.cells {
            let Some(Some((l, h))) = self.draw_stream.block_draws.get(c).map(|p| p.world) else {
                continue;
            };
            for a in 0..3 {
                lo[a] = lo[a].min(l[a]);
                hi[a] = hi[a].max(h[a]);
            }
        }
        entry.lo = lo;
        entry.hi = hi;
        self.draw_stream.block_draw_sections.insert(sp, entry);
        true
    }

    /// Re-resolve the cached placement of the set at `pos`, if there is one.
    ///
    /// Called from every path that writes a cell's block or model state
    /// without dropping its set: a machine changing COSTUME (`swap_model_block`
    /// → `refresh_region`) keeps what it owns, and a replica's corrective delta
    /// restores a cell's state under a set the server has not cleared. A stale
    /// transform would draw the machine's liquid where it used to face.
    pub(in crate::world) fn refresh_block_draw_placement(&mut self, pos: IVec3) {
        if self.draw_stream.block_draws.is_empty() {
            return;
        }
        let Some(set) = self
            .draw_stream
            .block_draws
            .get(&pos)
            .map(|p| Arc::clone(&p.set))
        else {
            return;
        };
        let (to_world, _) = self.draw_placement(pos, &set);
        if self.draw_stream.block_draws[&pos].to_world == to_world {
            return;
        }
        self.insert_draw(pos, set);
    }

    pub fn block_draw_at(&self, pos: IVec3) -> Option<&BlockDraw> {
        self.draw_stream.block_draws.get(&pos).map(|p| &p.set)
    }

    /// Every live draw set with the light at its cell, for the frame's
    /// presentation gather. Sparse: almost every world has none, so this is an
    /// `is_empty` test in the common case rather than a walk.
    pub fn collect_block_draws(
        &self,
        view: &petramond_world::view_volume::ViewVolume,
        out: &mut Vec<BlockDrawInstance>,
    ) {
        out.clear();
        // Section-first: a whole section's sets are rejected on ONE box test,
        // and only a section the view keeps costs a per-set test. Both boxes
        // were resolved at store time, so a set that is not on screen costs no
        // transform, no eight-corner fold and no light sample.
        for entry in self.draw_stream.block_draw_sections.values() {
            if !view.aabb_visible(entry.lo.into(), entry.hi.into()) {
                continue;
            }
            for &pos in &entry.cells {
                let Some(placed) = self.draw_stream.block_draws.get(&pos) else {
                    continue;
                };
                let Some((mn, mx)) = placed.world else {
                    continue;
                };
                if !view.aabb_visible(mn.into(), mx.into()) {
                    continue;
                }
                let sky = self.skylight6_at_world(pos.x, pos.y, pos.z);
                let block = petramond_world::light::BlockLight6::from_x2(
                    self.blocklight_rgb_at_world(pos.x, pos.y, pos.z),
                );
                out.push(BlockDrawInstance {
                    pos,
                    set: Arc::clone(&placed.set),
                    transform: placed.to_world,
                    skylight: sky,
                    blocklight: block,
                });
            }
        }
    }

    /// The space a block's OWN geometry is authored in, as a world transform.
    ///
    /// For a MODEL block that is its FOOTPRINT space — the same coordinates
    /// the `.bbmodel` is authored in, turned by the placed facing — so a mod
    /// computes geometry against the model it can see in Blockbench and a
    /// furnace facing east puts its metal where its spout is. For anything
    /// else it is the cell, `0..1`.
    ///
    /// ONE rule with two consumers: the draw-set gather below, and the ABI's
    /// `BlockLocalToWorld`, which is what lets a mod ask for a world point off
    /// its own model instead of writing this transform out a second time.
    pub fn block_local_transform(&self, pos: IVec3) -> petramond_math::math::Mat4 {
        let block = petramond_world::block::Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        if let Some(kind) = block.model_kind() {
            let offset = self.model_offset_at(pos.x, pos.y, pos.z);
            let facing = self.model_facing_at(pos.x, pos.y, pos.z);
            let base = petramond_world::block_model::base_from_cell(pos, kind, offset, facing);
            return petramond_world::block_model::placement_transform(base, kind, facing);
        }
        petramond_math::math::Mat4::from_translation(petramond_math::math::Vec3::new(
            pos.x as f32,
            pos.y as f32,
            pos.z as f32,
        ))
    }

    /// Drop the set at `pos` — called from the sweep that forgets a cell's
    /// other block-entity records (so a broken machine takes its drawing with
    /// it) and from the ordinary block write (so a cell that became something
    /// ELSE does too; the write cleared the cell's KV for the same reason).
    ///
    /// The early return is what makes it free to call from the write path: in
    /// a world with no draw sets at all — nearly every world — it costs a
    /// length check rather than a hash.
    pub(in crate::world) fn forget_block_draw(&mut self, pos: IVec3) {
        if self.draw_stream.block_draws.is_empty() {
            return;
        }
        if self.remove_draw(pos) {
            self.log_block_draw(pos);
        }
    }

    /// Every draw set whose anchor cell lies in `sp`, as wire rows — the
    /// INITIAL state a streaming-in section carries, beside the per-tick
    /// delta lane that carries changes to it.
    pub fn section_block_draws(
        &self,
        sp: petramond_world::chunk::SectionPos,
    ) -> Vec<crate::net::protocol::BlockDrawEntry> {
        let Some(entry) = self.draw_stream.block_draw_sections.get(&sp) else {
            return Vec::new();
        };
        let (ox, oy, oz) = (sp.cx * 16, sp.cy * 16, sp.cz * 16);
        let mut out: Vec<crate::net::protocol::BlockDrawEntry> = entry
            .cells
            .iter()
            .filter_map(|p| {
                let placed = self.draw_stream.block_draws.get(p)?;
                let cell = petramond_world::chunk::section_idx(
                    (p.x - ox) as usize,
                    (p.y - oy) as usize,
                    (p.z - oz) as usize,
                ) as u16;
                Some((cell, placed.set.wire.clone()))
            })
            .collect();
        out.sort_unstable_by_key(|(cell, ..)| *cell);
        out
    }

    /// Drop every draw set inside an unloading section. Unlike a break this
    /// logs NOTHING: the cells are gone from the recipient's view too, and a
    /// delta for a section nobody holds is filtered out anyway.
    pub(in crate::world) fn forget_block_draws_in_section(
        &mut self,
        pos: petramond_world::chunk::SectionPos,
    ) {
        let Some(entry) = self.draw_stream.block_draw_sections.remove(&pos) else {
            return;
        };
        for cell in entry.cells {
            self.draw_stream.block_draws.remove(&cell);
        }
    }

    fn log_block_draw(&mut self, pos: IVec3) {
        if self.replication.replication_capture {
            self.replication.block_draw_log.insert(pos);
        }
    }

    /// Drain this tick's changed draw sets as whole-set wire rows, sorted so
    /// the batch is deterministic.
    pub fn take_block_draw_deltas(&mut self) -> Vec<crate::net::protocol::BlockDrawDelta> {
        let mut out: Vec<_> = self
            .replication
            .block_draw_log
            .drain()
            .map(|pos| {
                let set = self.draw_stream.block_draws.get(&pos);
                crate::net::protocol::BlockDrawDelta {
                    pos,
                    prims: set.map(|p| p.set.wire.clone()).unwrap_or_default(),
                }
            })
            .collect();
        out.sort_unstable_by_key(|d| (d.pos.x, d.pos.y, d.pos.z));
        out
    }

    /// Apply one replicated row on the REPLICA.
    pub fn apply_remote_block_draw(&mut self, pos: IVec3, prims: DrawPrims) {
        if prims.is_empty() {
            self.remove_draw(pos);
        } else {
            self.insert_draw(pos, Arc::new(BlockDrawSet::new(prims)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::facing::Facing;
    use petramond_world::block::Block;
    use petramond_world::block_model::BlockModelKind;
    use petramond_world::chunk::{Chunk, ChunkPos, SectionPos};

    const WB: Block = Block::FurnitureWorkbench;

    fn prims() -> DrawPrims {
        DrawPrims::from(vec![DrawPrim::Cuboid {
            min: [0.25, 0.25, 0.25],
            max: [0.75, 0.75, 0.75],
            tile: "stone".into(),
            tint: [255, 255, 255],
            emissive: false,
        }])
    }

    fn world() -> World {
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    /// The frame gather reads a CACHED prim→world transform (that is what
    /// makes it cost nothing per off-screen set), so every path that rewrites
    /// a cell WITHOUT dropping its set has to re-read it. A stale one draws a
    /// machine's contents where the machine used to face, and culls it against
    /// a box it no longer occupies.
    #[test]
    fn a_stored_draws_placement_follows_its_cell() {
        let mut w = world();
        let base = petramond_world::block_model::base_from_front_left_anchor(
            IVec3::new(6, 64, 6),
            BlockModelKind::FurnitureWorkbench,
            Facing::East,
        );
        assert!(w.place_model_block_facing(base, WB, Facing::East));
        let anchor = w.container_anchor(base);
        w.set_block_draw(anchor, prims());
        let before = w.draw_stream.block_draws[&anchor].to_world;
        assert_eq!(before, w.block_local_transform(anchor), "stored fresh");

        // The cell turns under a surviving set — exactly what a costume swap
        // does to a machine that keeps its drawing.
        let cells = w.model_group(base).expect("a placed group").2;
        for c in &cells {
            let (chunk, lx, ly, lz) = w
                .chunk_at_world_mut(c.x, c.y, c.z)
                .expect("a placed footprint cell");
            chunk.set_model_facing(lx, ly, lz, Facing::North);
        }
        w.refresh_region(&cells);
        let after = w.draw_stream.block_draws[&anchor].to_world;
        assert_ne!(before, after, "fixture: the placement must actually move");
        assert_eq!(after, w.block_local_transform(anchor), "refreshed");
    }

    /// A delta lane is filtered PER RECIPIENT, so what a machine's redraw
    /// costs is multiplied by the player count. The prims therefore have to be
    /// SHARED with the stored set rather than copied out of it — with a `Vec`
    /// inside, twenty players watching one forge paid twenty deep copies of
    /// its prim list (heap `String` tile name per prim) every tick, and the
    /// section payload paid another on every stream-in.
    #[test]
    fn a_replicated_draw_set_is_shared_with_the_stored_one() {
        let mut w = world();
        let sp = SectionPos::from_world(4, 64, 4).expect("in range");
        let at = IVec3::new(4, 64, 4);
        w.set_block_world(at.x, at.y, at.z, Block::Stone);
        w.set_replication_capture(true);
        w.set_block_draw(at, prims());

        let stored = Arc::clone(&w.draw_stream.block_draws[&at].set);
        let deltas = w.take_block_draw_deltas();
        assert!(
            Arc::ptr_eq(&deltas[0].prims.0, &stored.wire.0),
            "the delta carries the stored allocation, not a copy of it"
        );
        let payload = w.section_block_draws(sp);
        assert!(
            Arc::ptr_eq(&payload[0].1 .0, &stored.wire.0),
            "and so does the section payload"
        );
    }

    /// The per-cell map and the per-section index are two views of one store;
    /// a cell left in one and not the other is either a set that draws forever
    /// or a section payload that ships a machine the world has forgotten.
    #[test]
    fn the_section_index_stays_in_step_with_the_cell_map() {
        let mut w = world();
        let sp = SectionPos::from_world(4, 64, 4).expect("in range");
        let cells: Vec<IVec3> = (0..3).map(|i| IVec3::new(4 + i, 64, 4)).collect();
        for &c in &cells {
            w.set_block_world(c.x, c.y, c.z, Block::Stone);
            w.set_block_draw(c, prims());
        }
        let indexed = |w: &World| {
            w.draw_stream
                .block_draw_sections
                .get(&sp)
                .map(|e| e.cells.len())
                .unwrap_or(0)
        };
        assert_eq!((w.draw_stream.block_draws.len(), indexed(&w)), (3, 3));
        assert_eq!(w.section_block_draws(sp).len(), 3);

        // Resubmitting a DIFFERENT set replaces in place — no duplicate index
        // entry, no second wire row in the section payload.
        let mut other = prims().as_slice().to_vec();
        if let DrawPrim::Cuboid { max, .. } = &mut other[0] {
            max[1] = 0.9;
        }
        w.set_block_draw(cells[0], other.into());
        assert_eq!((w.draw_stream.block_draws.len(), indexed(&w)), (3, 3));

        // A clear, then a block change, then the section going away.
        w.set_block_draw(cells[0], DrawPrims::default());
        assert_eq!((w.draw_stream.block_draws.len(), indexed(&w)), (2, 2));
        w.set_block_world(cells[1].x, cells[1].y, cells[1].z, Block::Air);
        assert_eq!((w.draw_stream.block_draws.len(), indexed(&w)), (1, 1));
        w.forget_block_draws_in_section(sp);
        assert_eq!((w.draw_stream.block_draws.len(), indexed(&w)), (0, 0));
        assert!(w.section_block_draws(sp).is_empty());
    }
}
