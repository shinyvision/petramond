use crate::world::{ReplicaWorld, ServerWorld, World, WorldSide};
use std::sync::Arc;

use mod_api::DrawPrim;

use petramond_math::math::IVec3;

/// A submitted prim list, SHARED. One block's set is stored once and every
/// consumer after that — the section payload, the per-tick delta, and one copy
/// per recipient session — takes a refcount bump.
///
/// It is a newtype rather than a bare `Arc<[DrawPrim]>` because it crosses the
/// wire, where the shape is an ordinary sequence (serde's `Arc` impls are
/// behind a feature the workspace does not take, and the local connection must
/// keep the refcount rather than round-trip). Same trade as
/// [`SectionBytes`](crate::world::replication::SectionBytes): in-process is free,
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

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BodyDraw {
    pub prims: DrawPrims,
    pub turns: bool,
}

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
    Sprite {
        at: [f32; 3],
        scale: f32,
        yaw: f32,
        pitch: f32,
        spin: f32,
        bob: [f32; 2],
        faces_viewer: bool,
        tile: petramond_world::tile::Tile,
        tint: [u8; 3],
        emissive: bool,
    },
}

impl BlockDrawPrim {
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
            DrawPrim::Sprite {
                at,
                scale,
                yaw,
                pitch,
                spin,
                bob,
                faces_viewer,
                tile,
                tint,
                emissive,
            } => {
                let tile = petramond_world::tile::Tile::from_name(tile)?;
                let sane = *scale > 0.0
                    && [*scale, *yaw, *pitch, *spin]
                        .iter()
                        .chain(at)
                        .chain(bob)
                        .all(|v| v.is_finite());
                sane.then_some(BlockDrawPrim::Sprite {
                    at: *at,
                    scale: *scale,
                    yaw: *yaw,
                    pitch: *pitch,
                    spin: *spin,
                    bob: *bob,
                    faces_viewer: *faces_viewer,
                    tile,
                    tint: *tint,
                    emissive: *emissive,
                })
            }
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct BlockDrawSet {
    pub wire: DrawPrims,
    pub resolved: Vec<BlockDrawPrim>,
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockLocalFrame {
    pub anchor: IVec3,
    pub transform: petramond_math::math::Mat4,
}

impl BlockLocalFrame {
    pub fn to_world(&self, p: petramond_math::math::Vec3) -> petramond_math::world_pos::WorldPos {
        petramond_math::world_pos::WorldPos::block_min(self.anchor)
            + self.transform.transform_point3(p)
    }

    pub fn relative_to(&self, origin: IVec3) -> petramond_math::math::Mat4 {
        petramond_math::math::Mat4::from_translation((self.anchor - origin).as_vec3())
            * self.transform
    }
}

pub fn world_bounds(
    frame: &BlockLocalFrame,
    lo: [f32; 3],
    hi: [f32; 3],
) -> (
    petramond_math::world_pos::WorldPos,
    petramond_math::world_pos::WorldPos,
) {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for cx in [lo[0], hi[0]] {
        for cy in [lo[1], hi[1]] {
            for cz in [lo[2], hi[2]] {
                let p = frame
                    .transform
                    .transform_point3(petramond_math::math::Vec3::new(cx, cy, cz));
                for a in 0..3 {
                    mn[a] = mn[a].min(p[a]);
                    mx[a] = mx[a].max(p[a]);
                }
            }
        }
    }
    let base = petramond_math::world_pos::WorldPos::block_min(frame.anchor);
    (base + mn.into(), base + mx.into())
}

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
            BlockDrawPrim::Sprite { at, scale, bob, .. } => {
                let r = scale * 0.87;
                let lift = bob[0].abs();
                grow(
                    [at[0] - r, at[1] - r - lift, at[2] - r],
                    [at[0] + r, at[1] + r + lift, at[2] + r],
                );
            }
            BlockDrawPrim::Item { at, scale, .. } => {
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

pub type DrawBox = ([f32; 3], [f32; 3]);

pub(in crate::world) struct PlacedDraw {
    pub(in crate::world) set: BlockDraw,
    pub(in crate::world) frame: BlockLocalFrame,
    pub(in crate::world) world: Option<(
        petramond_math::world_pos::WorldPos,
        petramond_math::world_pos::WorldPos,
    )>,
}

#[derive(Clone, Debug)]
pub struct BlockDrawInstance {
    pub pos: IVec3,
    pub set: BlockDraw,
    pub frame: BlockLocalFrame,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Default)]
pub(in crate::world) struct SectionDraws {
    cells: Vec<IVec3>,
    lo: petramond_math::world_pos::WorldPos,
    hi: petramond_math::world_pos::WorldPos,
}

impl SectionDraws {
    fn empty() -> SectionDraws {
        SectionDraws {
            cells: Vec::new(),
            lo: petramond_math::world_pos::WorldPos::new(f64::MAX, f64::MAX, f64::MAX),
            hi: petramond_math::world_pos::WorldPos::new(f64::MIN, f64::MIN, f64::MIN),
        }
    }

    fn grow(
        &mut self,
        world: Option<(
            petramond_math::world_pos::WorldPos,
            petramond_math::world_pos::WorldPos,
        )>,
    ) {
        let Some((lo, hi)) = world else { return };
        self.lo = petramond_math::world_pos::WorldPos::new(
            self.lo.x.min(lo.x),
            self.lo.y.min(lo.y),
            self.lo.z.min(lo.z),
        );
        self.hi = petramond_math::world_pos::WorldPos::new(
            self.hi.x.max(hi.x),
            self.hi.y.max(hi.y),
            self.hi.z.max(hi.z),
        );
    }
}

impl<S: WorldSide> World<S> {
    pub fn set_block_draw(&mut self, pos: IVec3, prims: DrawPrims) {
        let pos = self.container_anchor(pos);
        if self
            .draws
            .block_draws
            .get(&pos)
            .is_some_and(|s| s.set.wire == prims)
        {
            return;
        }
        self.store_draw(pos, prims);
    }

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

    fn draw_placement(
        &self,
        pos: IVec3,
        set: &BlockDrawSet,
    ) -> (
        BlockLocalFrame,
        Option<(
            petramond_math::world_pos::WorldPos,
            petramond_math::world_pos::WorldPos,
        )>,
    ) {
        let frame = self.block_local_frame(pos);
        let world = set.bounds.map(|(lo, hi)| world_bounds(&frame, lo, hi));
        (frame, world)
    }

    fn insert_draw(&mut self, pos: IVec3, set: BlockDraw) {
        let Some(sp) = petramond_world::chunk::SectionPos::from_world(pos.x, pos.y, pos.z) else {
            return;
        };
        let (frame, world) = self.draw_placement(pos, &set);
        let fresh = self
            .draws
            .block_draws
            .insert(pos, PlacedDraw { set, frame, world })
            .is_none();
        let entry = self
            .draws
            .block_draw_sections
            .entry(sp)
            .or_insert_with(SectionDraws::empty);
        if fresh {
            entry.cells.push(pos);
        }
        entry.grow(world);
    }

    fn remove_draw(&mut self, pos: IVec3) -> bool {
        if self.draws.block_draws.remove(&pos).is_none() {
            return false;
        }
        let Some(sp) = petramond_world::chunk::SectionPos::from_world(pos.x, pos.y, pos.z) else {
            return true;
        };
        let Some(mut entry) = self.draws.block_draw_sections.remove(&sp) else {
            return true;
        };
        entry.cells.retain(|c| *c != pos);
        if entry.cells.is_empty() {
            return true;
        }
        let mut refolded = SectionDraws::empty();
        for c in &entry.cells {
            refolded.grow(self.draws.block_draws.get(c).and_then(|p| p.world));
        }
        entry.lo = refolded.lo;
        entry.hi = refolded.hi;
        self.draws.block_draw_sections.insert(sp, entry);
        true
    }

    pub(in crate::world) fn refresh_block_draw_placement(&mut self, pos: IVec3) {
        if self.draws.block_draws.is_empty() {
            return;
        }
        let Some(set) = self.draws.block_draws.get(&pos).map(|p| Arc::clone(&p.set)) else {
            return;
        };
        let (frame, _) = self.draw_placement(pos, &set);
        if self.draws.block_draws[&pos].frame == frame {
            return;
        }
        self.insert_draw(pos, set);
    }

    pub fn block_draw_at(&self, pos: IVec3) -> Option<&BlockDraw> {
        self.draws.block_draws.get(&pos).map(|p| &p.set)
    }

    pub fn collect_block_draws(
        &self,
        view: &petramond_math::view_volume::ViewVolume,
        out: &mut Vec<BlockDrawInstance>,
    ) {
        out.clear();
        for entry in self.draws.block_draw_sections.values() {
            if !view.aabb_visible(entry.lo, entry.hi) {
                continue;
            }
            for &pos in &entry.cells {
                let Some(placed) = self.draws.block_draws.get(&pos) else {
                    continue;
                };
                let Some((mn, mx)) = placed.world else {
                    continue;
                };
                if !view.aabb_visible(mn, mx) {
                    continue;
                }
                let sky = self.data.skylight6_at_world(pos.x, pos.y, pos.z);
                let block = petramond_world::light::BlockLight6::from_x2(
                    self.data.blocklight_rgb_at_world(pos.x, pos.y, pos.z),
                );
                out.push(BlockDrawInstance {
                    pos,
                    set: Arc::clone(&placed.set),
                    frame: placed.frame,
                    skylight: sky,
                    blocklight: block,
                });
            }
        }
    }

    pub fn block_local_frame(&self, pos: IVec3) -> BlockLocalFrame {
        let block =
            petramond_world::block::Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        if let Some(kind) = block.model_kind() {
            let offset = self.data.model_offset_at(pos.x, pos.y, pos.z);
            let facing = self.data.model_facing_at(pos.x, pos.y, pos.z);
            return BlockLocalFrame {
                anchor: petramond_world::block_model::base_from_cell(pos, kind, offset, facing),
                transform: petramond_world::block_model::placement_transform(kind, facing),
            };
        }
        BlockLocalFrame {
            anchor: pos,
            transform: petramond_math::math::Mat4::IDENTITY,
        }
    }

    pub(in crate::world) fn forget_block_draw(&mut self, pos: IVec3) {
        if self.draws.block_draws.is_empty() {
            return;
        }
        if self.remove_draw(pos) {
            self.log_block_draw(pos);
        }
    }

    pub fn section_block_draws(
        &self,
        sp: petramond_world::chunk::SectionPos,
    ) -> Vec<crate::world::replication::BlockDrawEntry> {
        let Some(entry) = self.draws.block_draw_sections.get(&sp) else {
            return Vec::new();
        };
        let (ox, oy, oz) = (sp.cx * 16, sp.cy * 16, sp.cz * 16);
        let mut out: Vec<crate::world::replication::BlockDrawEntry> = entry
            .cells
            .iter()
            .filter_map(|p| {
                let placed = self.draws.block_draws.get(p)?;
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

    pub(in crate::world) fn forget_block_draws_in_section(
        &mut self,
        pos: petramond_world::chunk::SectionPos,
    ) {
        let Some(entry) = self.draws.block_draw_sections.remove(&pos) else {
            return;
        };
        for cell in entry.cells {
            self.draws.block_draws.remove(&cell);
        }
    }

    fn log_block_draw(&mut self, pos: IVec3) {
        if let Some(server) = self.side.server_mut() {
            if server.replication.replication_capture {
                server.replication.block_draw_log.insert(pos);
            }
        }
    }
}

impl ServerWorld {
    pub fn take_block_draw_deltas(&mut self) -> Vec<crate::world::replication::BlockDrawDelta> {
        let mut out: Vec<_> = self
            .side
            .replication
            .block_draw_log
            .drain()
            .map(|pos| {
                let set = self.draws.block_draws.get(&pos);
                crate::world::replication::BlockDrawDelta {
                    pos,
                    prims: set.map(|p| p.set.wire.clone()).unwrap_or_default(),
                }
            })
            .collect();
        out.sort_unstable_by_key(|d| (d.pos.x, d.pos.y, d.pos.z));
        out
    }
}

impl ReplicaWorld {
    pub fn apply_remote_block_draw(&mut self, pos: IVec3, prims: DrawPrims) {
        if let Some(section) = petramond_world::chunk::SectionPos::from_world(pos.x, pos.y, pos.z)
            .filter(|s| self.data.sections.contains_key(s))
        {
            self.before_section_write(section);
        }
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

    fn world() -> ServerWorld {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    /// The frame gather reads a CACHED prim→world transform (that is what
    /// makes it cost nothing per off-screen set), so every path that rewrites
    /// a cell WITHOUT dropping its set has to re-read it. A stale one draws a
    /// machine's contents at a stale facing, and culls it against
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
        let before = w.draws.block_draws[&anchor].frame;
        assert_eq!(before, w.block_local_frame(anchor), "stored fresh");

        let cells = w.model_group(base).expect("a placed group").2;
        let mut changes = Vec::new();
        for c in &cells {
            let (chunk, lx, ly, lz) = w
                .data
                .chunk_at_world_mut(c.x, c.y, c.z)
                .expect("a placed footprint cell");
            chunk.set_model_facing(lx, ly, lz, Facing::North);
            changes.push(crate::world::cell_change::CellChange::new(
                *c,
                chunk.block(lx, ly, lz),
                crate::world::cell_change::ChangeKind::Costume,
            ));
        }
        w.apply_cell_changes(&changes);
        let after = w.draws.block_draws[&anchor].frame;
        assert_ne!(before, after, "fixture: the placement must actually move");
        assert_eq!(after, w.block_local_frame(anchor), "refreshed");
    }

    #[test]
    fn a_replicated_draw_set_is_shared_with_the_stored_one() {
        let mut w = world();
        let sp = SectionPos::from_world(4, 64, 4).expect("in range");
        let at = IVec3::new(4, 64, 4);
        w.set_block_world(at.x, at.y, at.z, Block::Stone);
        w.set_replication_capture(true);
        w.set_block_draw(at, prims());

        let stored = Arc::clone(&w.draws.block_draws[&at].set);
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

    #[test]
    fn the_section_index_stays_in_step_with_the_cell_map() {
        let mut w = world();
        let sp = SectionPos::from_world(4, 64, 4).expect("in range");
        let cells: Vec<IVec3> = (0..3).map(|i| IVec3::new(4 + i, 64, 4)).collect();
        for &c in &cells {
            w.set_block_world(c.x, c.y, c.z, Block::Stone);
            w.set_block_draw(c, prims());
        }
        let indexed = |w: &ServerWorld| {
            w.draws
                .block_draw_sections
                .get(&sp)
                .map(|e| e.cells.len())
                .unwrap_or(0)
        };
        assert_eq!((w.draws.block_draws.len(), indexed(&w)), (3, 3));
        assert_eq!(w.section_block_draws(sp).len(), 3);

        let mut other = prims().as_slice().to_vec();
        if let DrawPrim::Cuboid { max, .. } = &mut other[0] {
            max[1] = 0.9;
        }
        w.set_block_draw(cells[0], other.into());
        assert_eq!((w.draws.block_draws.len(), indexed(&w)), (3, 3));

        w.set_block_draw(cells[0], DrawPrims::default());
        assert_eq!((w.draws.block_draws.len(), indexed(&w)), (2, 2));
        w.set_block_world(cells[1].x, cells[1].y, cells[1].z, Block::Air);
        assert_eq!((w.draws.block_draws.len(), indexed(&w)), (1, 1));
        w.forget_block_draws_in_section(sp);
        assert_eq!((w.draws.block_draws.len(), indexed(&w)), (0, 0));
        assert!(w.section_block_draws(sp).is_empty());
    }
}
