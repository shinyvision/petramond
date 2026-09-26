//! The non-cube render families' emitters: fluids, plant planes, the torch
//! pole, box sets and bbmodel blocks. Each reads the world only through the
//! mesher's [`Neighbourhood`] and lights through the shared lighting stage.

use glam::IVec3;
use petramond_world::block::{Block, PlantPlanes, ShapeBox};
use petramond_world::chunk::SKY_FULL;
use petramond_world::tile::Tile;

use super::super::boxset::{emit_box_set, snow_bed_boxes, BoxCell, BoxSetScratch, BoxWorld};
use super::super::face::Face;
use super::super::face_emit::FlatLit;
use super::super::vertex::Vertex;
use super::fluid_faces::{emit_fluid_cell, FluidStreams};
use super::lighting::{cell_light, face_lighting, CornerLight};
use super::mesher::{Cell, SectionMesher};
use super::model_block::{emit_model_block, emit_model_contact, ModelStreams, PlacedModelCell};
use super::neighbourhood::Neighbourhood;
use super::plant::emit_plant;

/// What a box-family cell left for the cube path.
pub(super) enum BoxesOutcome {
    /// The cell's box set was emitted.
    Drawn,
    /// The cell draws through the cube path: `whole_stack` when its resolved
    /// form IS the material's full cube (a uniform full slab stack), which
    /// greedy-merges like any opaque cube; otherwise it resolved no boxes (an
    /// unbaked custom-shape cell) and the cube is the render fallback.
    Cube { whole_stack: bool },
}

/// The box-set emitter's world hooks for one cell, answered by the
/// neighbourhood.
struct CellBoxWorld<'n> {
    nb: &'n Neighbourhood<'n>,
    pos: IVec3,
    block: Block,
}

impl BoxWorld for CellBoxWorld<'_> {
    fn neighbour_solid(&self, face: Face) -> bool {
        self.nb.solid(self.pos + face.dir())
    }

    fn neighbour_boxes(&self, face: Face, out: &mut Vec<([f32; 3], [f32; 3])>) {
        self.nb
            .occupancy_boxes(self.pos + face.dir(), self.block, out)
    }

    fn matter(&self, cell: IVec3, lo: [f32; 3], hi: [f32; 3]) -> bool {
        self.nb.matter(cell, lo, hi)
    }

    fn face_light(&self, face: Face, front: IVec3, plane: f32, smooth: bool) -> CornerLight {
        face_lighting(self.nb, face, front, plane, smooth)
    }
}

/// Emit one cell's box set through the unified box-set emitter.
fn emit_cell_boxes(
    nb: &Neighbourhood<'_>,
    vbuf: &mut Vec<Vertex>,
    at: BoxCell,
    block: Block,
    boxes: &[ShapeBox],
    scratch: &mut BoxSetScratch,
) {
    let world = CellBoxWorld {
        nb,
        pos: at.cell,
        block,
    };
    emit_box_set(vbuf, at, boxes, scratch, &world);
}

impl SectionMesher<'_> {
    /// Collect the snow blanket this cell is drawn standing in into
    /// `boxes.bed`, if the row asks for one and snow still touches it (see
    /// `boxset::snow_bed_boxes`). Ground decoration and a blanket compete for
    /// one cell, and the decoration always wins it, so without this every
    /// tuft and pebble is a bare hole in the white.
    fn collect_snow_bed(&mut self, cell: &Cell) {
        self.boxes.bed.clear();
        // Dense flag first: this runs for every drawn cell of a bedding
        // family, and almost none of them is decoration.
        if cell.block.is_snow_bedded() {
            let tints = &self.tints;
            snow_bed_boxes(
                &self.nb,
                cell.world,
                cell.block,
                &|t: Tile| tints.tile(t.world_tint(), cell.column),
                &mut self.boxes.bed,
            );
        }
    }

    pub(super) fn emit_fluid(&mut self, cell: &Cell) {
        let tints = &self.tints;
        emit_fluid_cell(
            &self.nb,
            FluidStreams {
                opaque: &mut self.out.opaque,
                transparent: &mut self.out.transparent,
                transparent_two_sided: &mut self.out.transparent_two_sided,
            },
            cell,
            self.anchor,
            |kind| tints.tile(kind, cell.column),
            tints.part(cell.idx, 0),
        );
    }

    /// Billboard plant planes, flat-lit from the cell, with the row's
    /// parameterized plane dimensions (a mod's retuned cross/crop) or the
    /// engine defaults for a parameterless row.
    pub(super) fn emit_plant(&mut self, cell: &Cell, layout: PlantPlanes) {
        self.collect_snow_bed(cell);
        // The bed is its own set here — a plant has no boxes to share one
        // with. Its planes are diagonal or inset, so nothing lands coplanar
        // with the blanket's top; the stalk's buried base is simply inside
        // opaque snow.
        if !self.boxes.bed.is_empty() {
            emit_cell_boxes(
                &self.nb,
                &mut self.out.opaque,
                BoxCell {
                    cell: cell.world,
                    anchor: self.anchor,
                },
                cell.block,
                &self.boxes.bed,
                &mut self.boxes.scratch,
            );
        }
        let block = cell.block;
        let tile = block.tiles()[0];
        let (sky6, blight) = cell_light(&self.nb, cell.world);
        let tint = self.tints.tile(tile.world_tint(), cell.column);
        let dims = block.shape_kind_def().params.dimensions();
        let (inset, drop) = match layout {
            PlantPlanes::Crop => (
                dims.map_or(petramond_world::block::CROP_PLANE_INSET, |d| d.inset),
                dims.map_or(petramond_world::block::CROP_PLANE_DROP, |d| d.drop),
            ),
            PlantPlanes::Cross => (dims.map_or(0.0, |d| d.inset), 0.0),
        };
        emit_plant(
            &mut self.out.opaque,
            layout,
            (cell.world - self.anchor).as_vec3(),
            tile,
            FlatLit {
                tint,
                sky6,
                block: blight,
            },
            inset,
            drop,
        );
    }

    /// The torch pole, posed by the cell's stored placement. Sky channel = the
    /// cell's skylight; block channel = the row's own emission (self-lit).
    /// `max(sky_term, block_term)` in the shader equals the old
    /// single-channel `max(cell_sky, emission)` fold at identity scale, and
    /// the emission channel never dims at night.
    pub(super) fn emit_pole(&mut self, cell: &Cell) {
        let block = cell.block;
        let [top_tile, _bottom, side_tile] = block.tiles();
        let cell_sky = u32::from(self.nb.skylight(cell.world));
        let sky6 = ((cell_sky * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
        let [er, eg, eb] = block.light_emission_rgb();
        let emit = petramond_world::light::BlockLight6::from_x2(
            petramond_world::light::LightRgb::new(er, eg, eb),
        );
        let placement = self.section.torch_placement(cell.lx, cell.ly, cell.lz);
        super::torch::emit_torch(
            &mut self.out.opaque,
            (cell.world - self.anchor).as_vec3(),
            placement,
            side_tile,
            top_tile,
            FlatLit {
                tint: [1.0, 1.0, 1.0],
                sky6,
                block: emit,
            },
        );
    }

    /// Every box-shaped family resolves through its own facet: ONE producer,
    /// so the drawn boxes are the boxes collision and targeting read. Adding
    /// a family means implementing `ShapeRender::boxes`, not editing the
    /// mesher.
    pub(super) fn emit_boxes(&mut self, cell: &Cell) -> BoxesOutcome {
        let block = cell.block;
        let kind = block.shape_kind_def();
        let whole_stack = {
            let tints = &self.tints;
            let tint_for = |tile: Tile| tints.tile(tile.world_tint(), cell.column);
            let cell_part_tint = |part| tints.part(cell.idx, part);
            let ctx = petramond_world::block::ShapeCtx {
                nb: &self.nb,
                pos: cell.world,
                block,
                params: &kind.params,
                tint_for: &tint_for,
                part_tint: &cell_part_tint,
            };
            // A family whose resolved form IS the material's full cube (a
            // uniform full slab stack) falls to the cube path so it
            // greedy-merges; the merge is load-bearing for streaming.
            let whole_stack = kind.render.meshes_as_cube(&ctx);
            if !whole_stack {
                self.boxes.cell.clear();
                kind.render.boxes(&ctx, &mut self.boxes.cell);
            }
            whole_stack
        };
        // Nothing resolved (an unbaked custom-shape cell) falls through to the
        // cube path — the render fallback.
        if whole_stack || self.boxes.cell.is_empty() {
            return BoxesOutcome::Cube { whole_stack };
        }
        self.tints.apply_to_boxes(&mut self.boxes.cell, cell.idx);
        // The bed joins the block's OWN set, and that is the whole reason this
        // is not a second emit: half the litter boxes are exactly one texel
        // tall, so their top face is coplanar with the blanket's. In one set
        // the emitter's coincidence tie-break settles which of the two draws
        // it — two sets would draw both and z-fight. Appended AFTER the cell
        // tint so a dyed decoration never dyes the snow it lies in.
        self.collect_snow_bed(cell);
        self.boxes.cell.extend_from_slice(&self.boxes.bed);
        emit_cell_boxes(
            &self.nb,
            &mut self.out.opaque,
            BoxCell {
                cell: cell.world,
                anchor: self.anchor,
            },
            block,
            &self.boxes.cell,
            &mut self.boxes.scratch,
        );
        BoxesOutcome::Drawn
    }

    /// A bbmodel block: its baked cell template, cullface-gated against the
    /// world neighbours, plus the contact shadow its bottom footprint stamps.
    pub(super) fn emit_model(&mut self, cell: &Cell) {
        let block = cell.block;
        let kind = block
            .model_kind()
            .expect("a Model-family row carries its bbmodel kind");
        let offset = self.section.model_offset(cell.lx, cell.ly, cell.lz);
        let facing = self.section.model_facing(cell.lx, cell.ly, cell.lz);
        let (sky6, blight) = cell_light(&self.nb, cell.world);
        let at = PlacedModelCell {
            kind,
            offset,
            facing,
            cell: cell.world,
            anchor: self.anchor,
        };
        let nb = &self.nb;
        emit_model_block(
            ModelStreams {
                verts: &mut self.out.model,
                indices: &mut self.out.model_idx,
                blend_indices: &mut self.out.model_blend_idx,
            },
            at,
            sky6,
            blight,
            self.tints.model_parts(cell.idx),
            self.tints.model_tint(cell.idx),
            // Cullface gate: the WORLD neighbour in the segment's direction
            // suppresses it when opaque (reads stay inside the ±1 mesh pad; an
            // unloaded neighbour reads as air and keeps the face).
            |f: Face| nb.block(cell.world + f.dir()).is_opaque(),
        );
        // Contact shadow: only a BOTTOM footprint cell stamps, each single-cell
        // piece (its own floor + its owned spill onto the dilation ring) gated
        // on ITS stamped cell — an opaque full cube directly below, and no
        // opaque full cube burying the floor at stamp level. Slabs, stairs,
        // lowered cubes, glass, other models, and air get no stamp —
        // supporting those shapes needs their real covered top surface and
        // height, not a relaxed opacity check.
        if offset[1] == 0 {
            let wy = cell.world.y;
            emit_model_contact(&mut self.out.contact, at, |gx, gz| {
                let below = nb.block(IVec3::new(gx, wy - 1, gz));
                if !below.is_cube_shaped() || !below.is_opaque() {
                    return false;
                }
                let at = nb.block(IVec3::new(gx, wy, gz));
                !at.is_cube_shaped() || !at.is_opaque()
            });
        }
    }
}
