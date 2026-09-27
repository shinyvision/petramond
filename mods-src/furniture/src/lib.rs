use mod_sdk::*;

mod cauldron;
mod chains;
mod keys;
mod lanterns;
mod seats;

use cauldron::{resolve_cauldron, Cauldron};
use chains::{resolve_chains, Chains};
use lanterns::{resolve_lanterns, Lanterns};
use seats::{release_broken_piece_sitters, ResolvedPiece};

fn is_strut(b: &ShapeAabb) -> bool {
    let mut dims = [0; 3].map(|_| 0.0f32);
    for (k, d) in dims.iter_mut().enumerate() {
        *d = b.max[k] - b.min[k];
    }
    dims.sort_by(|a, b| a.partial_cmp(b).expect("finite box"));
    dims[1] <= STRUT_SPAN
}

/// Slenderness, on the smaller two axes, at or under which a box is a strut.
const STRUT_SPAN: f32 = 2.0 / 16.0;

/// How dark the most enclosed strut is baked. The openness the rays measure is
/// NORMALIZED onto `DEEPEST..1.0` across the shape, so this is exactly the
/// contrast the shape shows — raw openness spans barely a tenth of its range on
/// a form as open as a chain, and used directly it bakes a uniform grey bar.
///
/// It is a multiply in LINEAR light, so the spread is `1.0 / DEEPEST`, and it
/// is a per-box multiply rather than per-texel paint — so it cannot produce the
/// row-on-row alternation a tile gradient does at any close range.
///
/// Vertex tint does not mipmap: viewed end-on down a receding run, the boxes
/// can alternate faster than a pixel. The 0.72 floor keeps contrast visible
/// nearby without adding speckling at distance.
const DEEPEST: f32 = 0.72;

const OCCLUSION_REACH: f32 = 8.0 / 16.0;

fn baked_occlusion(boxes: &[ShapeAabb], i: usize) -> Option<[u8; 3]> {
    if !is_strut(&boxes[i]) {
        return None;
    }
    let occluders = occluders(boxes);
    let raw: Vec<f32> = (0..boxes.len())
        .map(|k| openness(&occluders, &boxes[k]))
        .collect();
    let (lo, hi) = raw
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
    let span = hi - lo;
    let t = if span > 1e-4 {
        (raw[i] - lo) / span
    } else {
        1.0
    };
    let v = DEEPEST + (1.0 - DEEPEST) * t;
    Some([(v * 255.0).round().clamp(0.0, 255.0) as u8; 3])
}

fn openness(occluders: &[ShapeAabb], b: &ShapeAabb) -> f32 {
    let from = [0, 1, 2].map(|k| (b.min[k] + b.max[k]) * 0.5);
    let (mut open, mut cast) = (0u32, 0u32);
    for dx in -1..=1i32 {
        for dy in -1..=1i32 {
            for dz in -1..=1i32 {
                if (dx, dy, dz) == (0, 0, 0) {
                    continue;
                }
                let d = [dx as f32, dy as f32, dz as f32];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let dir = [d[0] / len, d[1] / len, d[2] / len];
                cast += 1;
                let blocked = occluders.iter().any(|o| o != b && ray_enters(from, dir, o));
                open += u32::from(!blocked);
            }
        }
    }
    open as f32 / cast as f32
}

/// The occluder set for a bake: the shape's own boxes, plus a copy one cell
/// either way along every axis the shape SPANS — touches both boundaries of.
///
/// A chain spans its cell, so in place it is a continuous run and its end
/// links are not open at all. Baking a lone cell says they are, which lifts
/// the boxes at both boundaries to the brightest value in the shape and paints
/// a lit band across every cell seam of every run — a repeating artifact, and
/// a worse one than the flat bar it was meant to fix.
fn occluders(boxes: &[ShapeAabb]) -> Vec<ShapeAabb> {
    let mut out = boxes.to_vec();
    for axis in 0..3 {
        let spans =
            boxes.iter().any(|b| b.min[axis] <= 0.0) && boxes.iter().any(|b| b.max[axis] >= 1.0);
        if !spans {
            continue;
        }
        for step in [-1.0f32, 1.0] {
            out.extend(boxes.iter().map(|b| {
                let mut c = *b;
                c.min[axis] += step;
                c.max[axis] += step;
                c
            }));
        }
    }
    out
}

fn ray_enters(from: [f32; 3], dir: [f32; 3], b: &ShapeAabb) -> bool {
    let (mut near, mut far) = (0.0f32, OCCLUSION_REACH);
    for k in 0..3 {
        if dir[k].abs() < 1e-6 {
            if from[k] < b.min[k] || from[k] > b.max[k] {
                return false;
            }
            continue;
        }
        let (a, c) = ((b.min[k] - from[k]) / dir[k], (b.max[k] - from[k]) / dir[k]);
        near = near.max(a.min(c));
        far = far.min(a.max(c));
    }
    near <= far
}

const ON_INTERACT: u32 = 1;
const ON_BLOCK_BROKEN: u32 = 2;

#[derive(Default)]
struct Furniture {
    pieces: Vec<ResolvedPiece>,
    chains: Option<Chains>,
    lanterns: Option<Lanterns>,
    cauldron: Option<Cauldron>,
    buckets: Option<WaterBuckets>,
    dyeables: Vec<(ItemId, String)>,
    pigments: Vec<(ItemId, [u8; 3], bool)>,
    client: bool,
}

impl Mod for Furniture {
    fn init(&mut self) {
        self.pieces = seats::load_pieces();
        self.client = runtime_side() == RuntimeSide::Client;
        self.chains = resolve_chains();
        self.lanterns = resolve_lanterns();
        self.cauldron = resolve_cauldron();
        self.buckets = WaterBuckets::resolve();
        self.dyeables = cauldron::load_dyeables();
        self.pigments = cauldron::load_pigments();
        register_event_handler(EventKind::InteractAttempt, 0, ON_INTERACT);
        if !self.client {
            register_event_handler(EventKind::BlockBroken, 0, ON_BLOCK_BROKEN);
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (
                ON_INTERACT,
                EventPayload::InteractAttempt {
                    block: Some(pos),
                    player,
                    ..
                },
            ) => {
                let actor = player_state();
                let claimed = if self.client {
                    let replica = SideWorld::Replica;
                    self.cauldron_claims(&replica, *pos, &actor)
                        || self.seat_gate(&replica, *pos, &actor).is_some()
                } else {
                    self.try_use_cauldron(*pos, &actor) || self.try_sit(*pos, *player, &actor)
                };
                if claimed {
                    Outcome::Cancel
                } else {
                    Outcome::Continue
                }
            }
            (ON_BLOCK_BROKEN, EventPayload::BlockBroken { pos, block, .. }) => {
                if let Some(resolved) = self.pieces.iter().find(|p| p.block == *block) {
                    release_broken_piece_sitters(resolved.block, &resolved.piece, *pos);
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn bake_shape_sim(&mut self, shape_kind: u16, cells: &[CellInput]) -> Vec<BakedSimCell> {
        if !self.owns_shape(shape_kind) {
            return Vec::new();
        }
        cells
            .iter()
            .map(|cell| BakedSimCell {
                collision_boxes: self.shape_boxes(shape_kind, cell.block_id),
                light_aperture: LightAperture::Open,
            })
            .collect()
    }

    fn bake_shape_render(&mut self, shape_kind: u16, cells: &[CellInput]) -> Vec<BakedRenderCell> {
        if !self.owns_shape(shape_kind) {
            return Vec::new();
        }
        let uses_at = self.cauldron_dye_uses(shape_kind, cells);
        let mut baked: Vec<(BlockId, Vec<ShapeRenderBox>)> = Vec::new();
        let mut out = Vec::with_capacity(cells.len());
        for cell in cells {
            if !baked.iter().any(|(id, _)| *id == cell.block_id) {
                let raw = self.shape_boxes(shape_kind, cell.block_id);
                let cooked = raw
                    .iter()
                    .enumerate()
                    .map(|(i, aabb)| {
                        let tint = baked_occlusion(&raw, i);
                        ShapeRenderBox {
                            tint,
                            ao: tint.map(|_| 0),
                            ..(*aabb).into()
                        }
                    })
                    .collect();
                baked.push((cell.block_id, cooked));
            }
            let mut boxes = baked
                .iter()
                .find(|(id, _)| *id == cell.block_id)
                .expect("just baked")
                .1
                .clone();
            boxes.extend(self.cauldron_fluid_box(shape_kind, cell, &uses_at));
            out.push(BakedRenderCell { boxes });
        }
        out
    }

    fn bake_shape_item(&mut self, shape_kind: u16, _block: BlockId) -> BakedItemGeometry {
        let boxes = if self.chains.as_ref().is_some_and(|c| c.shape == shape_kind) {
            chains::cell_links()
        } else if let Some(lanterns) = self.lanterns.as_ref().filter(|l| l.shape == shape_kind) {
            lanterns.item_boxes()
        } else if self
            .cauldron
            .as_ref()
            .is_some_and(|c| c.shape == shape_kind)
        {
            cauldron::CAULDRON_BOXES.to_vec()
        } else {
            Vec::new()
        };
        BakedItemGeometry { boxes }
    }

    fn shape_placement_plan(
        &mut self,
        shape_kind: u16,
        _block: BlockId,
        inputs: &PlaceInputsView,
    ) -> ShapePlacementResult {
        let row = self
            .chains
            .as_ref()
            .filter(|c| c.shape == shape_kind)
            .map(|c| c.row_for_normal(inputs.normal))
            .or_else(|| {
                self.lanterns
                    .as_ref()
                    .filter(|l| l.shape == shape_kind)
                    .map(|l| l.row_for_normal(inputs.normal))
            });
        ShapePlacementResult {
            accepted: true,
            anchor: inputs.place_pos,
            cells: vec![inputs.place_pos],
            block: row,
        }
    }
}

impl Furniture {
    fn owns_shape(&self, shape_kind: u16) -> bool {
        self.chains.as_ref().is_some_and(|c| c.shape == shape_kind)
            || self
                .lanterns
                .as_ref()
                .is_some_and(|l| l.shape == shape_kind)
            || self
                .cauldron
                .as_ref()
                .is_some_and(|c| c.shape == shape_kind)
    }

    fn shape_boxes(&self, shape_kind: u16, block: BlockId) -> Vec<ShapeAabb> {
        if let Some(chains) = self.chains.as_ref().filter(|c| c.shape == shape_kind) {
            return chains.links_for(block);
        }
        if let Some(lanterns) = self.lanterns.as_ref().filter(|l| l.shape == shape_kind) {
            return lanterns.boxes_for(block);
        }
        if self
            .cauldron
            .as_ref()
            .is_some_and(|c| c.shape == shape_kind)
        {
            return cauldron::CAULDRON_BOXES.to_vec();
        }
        Vec::new()
    }
}

mod_sdk::register_mod!(Furniture);

/// Cabinets are pack-only: the block row's `open_gui` kind has to match a container-class document
/// whose `container` slots size it. Nothing checks this at load. Rename the document or edit the
/// grid and storage gets skipped or undersized with just a stderr line, and GUI clicks go nowhere.
/// These tests are the only place we compare rows against documents.
#[cfg(test)]
mod cabinet_documents {
    use mod_sdk::json::Value;

    use crate::keys;

    const BLOCKS: &str = include_str!("../pack/blocks.json");
    const DOCUMENTS: &[(&str, &str)] = &[
        (
            keys::CABINET_GUI,
            include_str!("../pack/ui/documents/cabinet.gui.json"),
        ),
        (
            keys::COUNTER_CABINET_GUI,
            include_str!("../pack/ui/documents/counter_cabinet.gui.json"),
        ),
    ];

    fn slots_in_role(node: &Value, role: &str) -> usize {
        let mut n = match (
            node.get("type").and_then(Value::as_str),
            node.get("role").and_then(Value::as_str),
        ) {
            (Some("slot_grid"), Some(r)) if r == role => {
                let cols = node.get("cols").and_then(Value::as_u8).unwrap_or(1) as usize;
                let rows = node.get("rows").and_then(Value::as_u8).unwrap_or(1) as usize;
                cols * rows
            }
            (Some("slot"), Some(r)) if r == role => 1,
            _ => 0,
        };
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                n += slots_in_role(child, role);
            }
        }
        n
    }

    #[test]
    fn the_cabinet_rows_open_container_documents_sized_to_two_rows() {
        let blocks = Value::parse(BLOCKS).expect("pack/blocks.json parses");
        for (kind, doc_text) in DOCUMENTS {
            let doc = Value::parse(doc_text).expect("the shipped document parses");
            assert_eq!(
                doc.get("kind").and_then(Value::as_str),
                Some(*kind),
                "document kind must equal the block row's open_gui kind"
            );
            assert_eq!(
                doc.get("class").and_then(Value::as_str),
                Some("container"),
                "a storage container document is container-class"
            );
            let root = doc.get("root").expect("the document has a root node");
            assert_eq!(
                slots_in_role(root, "container"),
                18,
                "{kind}: 2 rows of 9 — resizing the grid changes the storage"
            );
            assert_eq!(slots_in_role(root, "player_inv"), 27, "{kind}");
            assert_eq!(slots_in_role(root, "hotbar"), 9, "{kind}");

            let row = blocks
                .get("blocks")
                .and_then(Value::as_array)
                .expect("blocks.json has a blocks list")
                .iter()
                .find(|b| b.get("block").and_then(Value::as_str) == Some(*kind))
                .unwrap_or_else(|| panic!("{kind} has a block row"));
            let opened = row
                .get("interaction")
                .and_then(|i| i.get("open_gui"))
                .and_then(Value::as_str)
                .expect("{kind} opens a GUI on click");
            assert_eq!(
                opened, *kind,
                "the row and the shipped document must name the SAME kind"
            );
        }
    }
}

#[cfg(test)]
mod bake_tests {
    use super::*;

    #[test]
    fn every_chain_link_is_a_strut() {
        let links = chains::cell_links();
        assert!(!links.is_empty());
        for (i, b) in links.iter().enumerate() {
            assert!(is_strut(b), "link box {i} is not a strut: {b:?}");
        }
    }

    #[test]
    fn a_plate_keeps_its_contact_shadow() {
        let plate = ShapeAabb {
            min: [4.0 / 16.0, 0.0, 4.0 / 16.0],
            max: [12.0 / 16.0, 1.0 / 16.0, 12.0 / 16.0],
        };
        assert!(!is_strut(&plate));
    }

    /// The bake must be PERIODIC along a run: link k and the link two above it
    /// are the same box in the same surroundings, so they must bake the same.
    ///
    /// They only do because the occluder set repeats the cell (`occluders`).
    /// Bake a lone cell and the boxes at both boundaries measure open, come out
    /// the brightest in the shape, and paint a lit band across every cell seam
    /// of every run — which is a repeating artifact rather than a subtle one,
    /// and no screenshot of a single block would ever show it.
    #[test]
    fn baked_occlusion_is_periodic_along_a_run() {
        let links = chains::cell_links();
        let per_link = 4;
        let bake = |i| baked_occlusion(&links, i).expect("every link bakes")[0];
        for i in 0..links.len() - 2 * per_link {
            assert_eq!(
                bake(i),
                bake(i + 2 * per_link),
                "box {i} and the matching box two links up bake differently"
            );
        }
        assert_eq!(
            bake(0),
            bake(2 * per_link),
            "the run's first link is special-cased"
        );
    }

    #[test]
    fn baked_occlusion_uses_its_range() {
        let links = chains::cell_links();
        let tints: Vec<u8> = (0..links.len())
            .filter_map(|i| baked_occlusion(&links, i).map(|t| t[0]))
            .collect();
        assert_eq!(tints.len(), links.len(), "every link should bake");
        let (lo, hi) = (
            *tints.iter().min().expect("links"),
            *tints.iter().max().expect("links"),
        );
        let spread = f32::from(hi) / f32::from(lo);
        let want = 1.0 / DEEPEST;
        assert!(
            (spread - want).abs() < 0.05,
            "baked spread {spread:.2}x should be DEEPEST's {want:.2}x"
        );
    }
}
