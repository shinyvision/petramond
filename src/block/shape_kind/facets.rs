//! The two shape facet traits — the composable seam a shape family implements
//! so consumers dispatch through it instead of matching a closed enum.
//!
//! The split is by AUDIENCE (see the plan / WIKI/modding.md): [`ShapeSim`] is
//! authoritative and deterministic (collision, support, nav — the multiplayer
//! tick reads it, on the server and re-evaluated against the client replica);
//! [`ShapeRender`] is client presentation (selection outline, item form). Both
//! read the world through `&World` directly — the client's replica IS a
//! `World` (`Game::replica`), so one type serves both sides and a shape behaves
//! identically on each by construction. A family unit struct implements one or
//! both; the [`ShapeKindDef`](super::ShapeKindDef) row binds the singletons.

use crate::atlas::Tile;
use crate::block_model::BlockModelKind;
use crate::mathh::IVec3;
use crate::world::World;

use super::super::{Aabb, Block};
use super::ShapeParams;

/// Sim-side shape behavior: authoritative, deterministic. A headless server
/// calls every method here and never touches [`ShapeRender`].
pub trait ShapeSim: Send + Sync + 'static {
    /// The block's position-aware collision boxes — the resolve behind
    /// [`World::collision_boxes_at`]. The default is the row's position-less
    /// [`Block::collision_boxes`] (right for cube/cross/crop/torch/lowered);
    /// stateful and neighbour-aware families override.
    fn collision_boxes(
        &self,
        _params: &ShapeParams,
        _world: &World,
        _pos: IVec3,
        block: Block,
    ) -> &'static [Aabb] {
        block.collision_boxes()
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
    /// real box. Default is the row's [`Block::visual_aabb`].
    fn selection_box(
        &self,
        _params: &ShapeParams,
        _world: &World,
        _pos: IVec3,
        block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        block.visual_aabb()
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
    /// A plain full-cube icon (cube, lowered cube).
    Cube(Block),
    /// True baked geometry built from the family + the held state (stair, slab,
    /// fence) — the same helpers the chunk mesher uses.
    Geometry(Block),
    /// A baked bbmodel, everywhere.
    Model(BlockModelKind),
}
