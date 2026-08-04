//! A pack's WASM-baked procedural shape; geometry comes from the bake caches.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A mod-defined procedural shape. Collision/geometry come from the
/// WASM bake cache; on a cache miss or a trapped bake it falls back to the row's
/// static collision/visual boxes (the failure policy). Nav solidity is a
/// declared property of the shape.
pub struct CustomFamily;

impl ShapeSim for CustomFamily {
    fn light_shape(&self, p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        // The declared light tier: the simple open/opaque_cube declarations,
        // or the per-cell aperture whose opacity the SIM bake supplies
        // through the light snapshot.
        match p.custom() {
            Some(c) if c.light_shape == crate::block::shape_kind::CustomLight::OpaqueCube => {
                crate::block::BlockLightShape::OpaqueCube
            }
            Some(c) if c.light_shape == crate::block::shape_kind::CustomLight::CustomAperture => {
                crate::block::BlockLightShape::Shaped
            }
            _ => crate::block::BlockLightShape::Open,
        }
    }

    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
    ) -> &'static [Aabb] {
        // The sim bake cache, or the row's static boxes on a miss / trapped bake.
        nb.baked_collision(pos)
            .unwrap_or_else(|| block.collision_boxes())
    }
    fn nav_reads_solid(&self, p: &ShapeParams) -> bool {
        p.custom().is_some_and(|c| c.nav_solid)
    }

    fn occupies_pocket(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        // The SIM bake's boxes: the authoritative matter, and the only cache
        // a headless light bake can reach. A render-only box (a fluid surface
        // inside a pot) deliberately casts nothing.
        nb.baked_collision(pos)
            .is_some_and(|boxes| boxes.iter().any(|bx| overlaps(lo, hi, bx.min, bx.max)))
    }

    /// Derived from the BAKED matter, like the box set's: a pack shape that
    /// fills a boundary holds a mount there, one that does not (a chain, a
    /// lantern) holds nothing — with no row field to keep in step and no
    /// family named. An unbaked cell answers `None`, the family's failure
    /// policy everywhere else too.
    fn full_face(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        dir: IVec3,
    ) -> Option<crate::block::shape_kind::facets::FullFace> {
        crate::block::shape_kind::facets::face_is_solid(nb, pos, dir)
            .then_some(crate::block::shape_kind::facets::FullFace::Shaped)
    }
}

impl ShapeRender for CustomFamily {
    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        // A custom shape draws what its WASM bake produced. No bake reachable
        // (never baked, trapped, or outside the caller's window) emits nothing,
        // and the caller falls back to the row's static form.
        let Some(baked) = ctx.nb.baked(ctx.pos) else {
            return;
        };
        let tiles = ctx.block.tiles();
        out.extend(baked.iter().map(|b| {
            // The bake's per-box tint multiplies the world tint; untinted
            // boxes carry [1.0; 3] and cost nothing.
            let tint_for = |tile: crate::tile::Tile| {
                let world = (ctx.tint_for)(tile);
                [
                    world[0] * b.tint[0],
                    world[1] * b.tint[1],
                    world[2] * b.tint[2],
                ]
            };
            let mut mb = ShapeBox::uniform(b.aabb, tiles, tint_for).with_ao_strength(b.ao_strength);
            // The bake DECLARES whether its tint is a dye (sample the
            // dye-base twin) or a plain hue-preserving multiply over the
            // authored texels — both are expressible, per the wire field.
            mb.dyed = b.dyed;
            mb
        }));
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        // The item KIND is true baked geometry: `render::item_cube`'s custom
        // branch draws the shape's `BakeShapeItem` boxes from the item cache
        // (cube fallback on a miss). Still a `BlockCube` render kind, so item
        // entities / in-hand / icon all route through the cube item renderer.
        ItemRender::BlockForm(block)
    }
}

impl ShapePlacement for CustomFamily {}
