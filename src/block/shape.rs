use serde::{Deserialize, Serialize};

use crate::atlas::Tile;

/// How far a crop plane ([`ShapeFamily::Crop`](super::ShapeFamily::Crop)) sits
/// in from the cell faces it is perpendicular to (2/16 of a block). Shared by
/// the mesher, the targeting ray, and the selection outline so they always
/// trace the same geometry.
pub const CROP_PLANE_INSET: f32 = 2.0 / 16.0;

/// How far a crop plane hangs BELOW its cell (1/16): the art's bottom row sits
/// on the sunken top of the farmland underneath (a lowered cube) instead of
/// floating a texel above it. On a full block (a wild crop on grass) the
/// overhang is buried and invisible.
pub const CROP_PLANE_DROP: f32 = 1.0 / 16.0;

/// How a block participates in light propagation. This is the render/collision-neutral
/// shape category that `world::light` consumes; per-cell state, such as stair facing,
/// still lives in the section and is interpreted by the lighting shape layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockLightShape {
    Open,
    OpaqueCube,
    /// The cell's light passage depends on its per-cell state: the light
    /// snapshot carries the FAMILY-answered packed per-face apertures
    /// (`ShapeSim::light_apertures` — stair steps, slab layers, a WASM
    /// shape's baked opacity) and the flood consults them with no family
    /// knowledge of its own.
    Shaped,
}

/// One axis-aligned box of a block's collision shape, in CELL-LOCAL coordinates
/// (`0.0..1.0` per axis). A block's full shape is a *list* of these (see
/// [`Block::collision_boxes`]) — one for a full cube or the inset chest, several for
/// shapes like stairs. The player collides via a swept-AABB over them, and the
/// selection outline + break overlay derive from their union.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// One DRAWN box of a custom shape's render bake: geometry plus the
/// per-box RGB multiply tint (`[1.0; 3]` = untinted, the ordinary case). The
/// engine form of the wire `mod_api::ShapeRenderBox`; the mesher multiplies
/// it into the box's per-vertex tint lane (the biome grass/water channel).
/// Render-side only — collision, selection, and targeting never see it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShapeRenderBox {
    pub aabb: Aabb,
    pub tint: [f32; 3],
    /// AO darkening strength for this box's faces (`1.0` = full, `0.0` =
    /// AO-immune) — the engine form of the wire percentage.
    pub ao_strength: f32,
    /// Sample the tiles' dye-base twins so `tint` lands on a peak-white base
    /// (see the wire field's doc) — an undyed tint stays a plain multiply
    /// over the authored texels.
    pub dyed: bool,
}

/// A cell's sub-cell PART id — the addressing unit for per-part cell data
/// (today `petramond:tint`, tomorrow whatever a consumer keys per part).
///
/// Part `0` is the WHOLE-CELL part: every family but the stacking slab has
/// exactly one, leaves this `0`, and addresses the bare un-suffixed KV keys,
/// so nothing about an existing save or a dyed cube changes.
///
/// A family that DOES have several parts owns the numbering and must use the
/// same numbers in [`super::ShapeRender::boxes`] ([`ShapeBox::part`]),
/// [`super::ShapeSim::parts`], and its placement plan's writes. That agreement
/// is the whole trick: it lets a courier address "the part this click filled"
/// or "the part this drop came from" while knowing nothing about the family.
pub type CellPart = u8;

/// How one face of a [`ShapeBox`] is textured.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShapeFace {
    pub tile: Tile,
    /// Swap the cell-local u/v (the pane edge strip laid along a W/E arm).
    pub swap_uv: bool,
    /// Quarter turns (`0..4`) to rotate the cell-local UV by, applied after
    /// [`swap_uv`](Self::swap_uv).
    ///
    /// A shape that turns about Y keeps the cell-local UV of its four SIDE
    /// faces for free — the rotation carries a face's art to the next
    /// direction unchanged — but its top and bottom faces sample the same
    /// fixed tile through a turned footprint, so their art would stay pinned
    /// to world north while the block turns. This is the correction, and it is
    /// the only reason a tile can be authored once for all four facings
    /// instead of four times.
    pub uv_turns: u8,
    pub tint: [f32; 3],
}

impl ShapeFace {
    /// Turn a cell-local UV by `turns` quarter turns — the transform
    /// [`uv_turns`](Self::uv_turns) names, shared by every consumer that
    /// samples a face (chunk mesh, item cube) so they cannot disagree.
    #[inline]
    pub fn turn_uv(turns: u8, u: f32, v: f32) -> (f32, f32) {
        match turns & 3 {
            1 => (v, 1.0 - u),
            2 => (1.0 - u, 1.0 - v),
            3 => (1.0 - v, u),
            _ => (u, v),
        }
    }
}

/// One cell-local cuboid of a shape's ITEM form — what the icon, the dropped
/// entity, and the in-hand model draw.
///
/// Deliberately NOT [`ShapeBox`]: an item is not always the placed form. A
/// fence item is an authored two-post SEGMENT, where the placed cell with no
/// neighbours resolves to a bare post. The family owns that difference, which
/// is why it answers [`ShapeRender::item_boxes`] instead of the renderer
/// re-deriving a form per family.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemBox {
    pub aabb: Aabb,
    /// Which faces the item draws. A face the family omits is one it knows is
    /// buried (a fence rail's end against its post).
    pub faces: [bool; 6],
    /// The block whose icon tiles this box samples — a stacked slab's two
    /// layers can be different materials. `None` = the item's own block.
    pub material: Option<crate::block::Block>,
    /// Per-face tile override (a static box set's authored tiles); `None`
    /// falls back to the material's icon face tile.
    pub tiles: [Option<crate::atlas::Tile>; 6],
    /// Per-face extra UV quarter-turns (a static box set's `uv_turns`).
    pub uv_turns: [u8; 6],
}

impl ItemBox {
    /// A box drawing all six faces from the item's own block, untinted and
    /// unturned — what stair/slab/fence forms want.
    pub fn solid(min: [f32; 3], max: [f32; 3]) -> Self {
        ItemBox {
            aabb: Aabb { min, max },
            faces: [true; 6],
            material: None,
            tiles: [None; 6],
            uv_turns: [0; 6],
        }
    }
}

/// One cell-local cuboid of a RESOLVED block shape — the family-agnostic
/// geometry currency.
///
/// Every shape family answers with these and every consumer reads them: the
/// chunk mesher, the collision sweep, the targeting ray, the selection
/// outline, and the neighbour-occupancy cull. That is the whole point — the
/// drawn boxes, the collided boxes and the aimed boxes are ONE list, so they
/// cannot drift the way six independent per-family producers did.
///
/// Presentation lives on the box (`faces`, `dyed`, `ao_strength`) and the sim
/// consumers simply ignore it; a headless server resolves the same boxes and
/// never looks at a tile.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShapeBox {
    pub aabb: Aabb,
    /// Indexed in canonical face order (`+X, -X, +Y, -Y, +Z, -Z`). `None` =
    /// the family NEVER emits that face, whatever the geometry says — a fence
    /// rail's end cap and a pane's connected end are guaranteed covered by the
    /// connection RULE, not by local geometry, so subtraction alone could not
    /// prove them hidden.
    pub faces: [Option<ShapeFace>; 6],
    /// How strongly AO darkens this box's faces: `1.0` = the ordinary full
    /// effect, `0.0` = AO-immune. Scales the DARKENING within the 0..3
    /// vertex-AO vocabulary, so a fluid surface inside a pot can sit bright
    /// while the pot itself keeps its pockets and creases.
    pub ao_strength: f32,
    /// The box's faces sample their tiles' dye-base twins — set wherever a
    /// `petramond:tint` multiply applies, so the tint lands on a peak-white
    /// base and can whiten as well as dye.
    pub dyed: bool,
    /// Which sub-cell [`CellPart`] this box belongs to, so the mesher's tint
    /// post-pass can give each part its own multiply. `0` for every
    /// single-part family.
    pub part: CellPart,
    /// Whether this box is MATTER — part of the block's body — rather than a
    /// bare carrier for a face. Matter shadows, blocks light, buries a
    /// neighbour's face and hides what is behind it; a face carrier does none
    /// of that. The cactus's side planes span the whole cell so their faces
    /// come out full width, but the body they show is the inset trunk: count
    /// them as matter and the cactus shadows the ground like a full cube,
    /// seals its own cell dark, and punches holes in the faces of whatever
    /// stands beside it.
    pub occludes: bool,
    /// Draw this box's faces from BOTH sides (the back winding too), so they
    /// survive back-face culling.
    ///
    /// For a cutout face this is what keeps the art whole from every angle:
    /// the cactus's side tile is transparent along its edge columns EXCEPT
    /// where the spines poke out, so through the near face's notches you see
    /// the far face's spines instead of the sky. Single-sided, half the spines
    /// vanish the moment you strafe past.
    pub double_sided: bool,
}

impl ShapeBox {
    /// The neutral box: no geometry, no faces, ordinary matter. The base a
    /// family fills in with `..ShapeBox::PLAIN` when it builds per-face styles
    /// by hand, so a new presentation field does not touch every producer.
    pub const PLAIN: ShapeBox = ShapeBox {
        aabb: Aabb {
            min: [0.0; 3],
            max: [1.0; 3],
        },
        faces: [None; 6],
        ao_strength: 1.0,
        dyed: false,
        part: 0,
        occludes: true,
        double_sided: false,
    };

    /// A box textured like a cube: `[top, bottom, side]` tiles, one tint per
    /// tile, plain UVs on every face, full AO, undyed.
    pub fn uniform(aabb: Aabb, tiles: [Tile; 3], tint_for: impl Fn(Tile) -> [f32; 3]) -> Self {
        let style = |tile: Tile| {
            Some(ShapeFace {
                tile,
                swap_uv: false,
                uv_turns: 0,
                tint: tint_for(tile),
            })
        };
        // Canonical face order: +X, -X, +Y, -Y, +Z, -Z.
        let mut faces = [style(tiles[2]); 6];
        faces[2] = style(tiles[0]);
        faces[3] = style(tiles[1]);
        ShapeBox {
            aabb,
            faces,
            ao_strength: 1.0,
            dyed: false,
            part: 0,
            occludes: true,
            double_sided: false,
        }
    }

    /// The same box drawn from both sides (see
    /// [`double_sided`](Self::double_sided)).
    pub fn double_sided(mut self) -> Self {
        self.double_sided = true;
        self
    }

    /// The same box as a bare FACE CARRIER rather than matter (see
    /// [`occludes`](Self::occludes)).
    pub fn as_face_carrier(mut self) -> Self {
        self.occludes = false;
        self
    }

    /// The same box with its AO darkening scaled (`1.0` = default).
    pub fn with_ao_strength(mut self, strength: f32) -> Self {
        self.ao_strength = strength;
        self
    }

    /// The same box assigned to a sub-cell [`CellPart`] — a multi-part family
    /// tags each box with the part it draws so the tint post-pass can address
    /// them separately.
    pub fn with_part(mut self, part: CellPart) -> Self {
        self.part = part;
        self
    }

    /// Multiply a presentation tint into every emitted face and mark the box
    /// dyed, so the multiply lands on the tile's dye-base twin. The ONE place
    /// a cell tint reaches box geometry — every family gets it by construction
    /// instead of each remembering to fold it into its own tint closure.
    pub fn apply_tint(&mut self, tint: [f32; 3]) {
        self.dyed = true;
        for face in self.faces.iter_mut().flatten() {
            face.tint = [
                face.tint[0] * tint[0],
                face.tint[1] * tint[1],
                face.tint[2] * tint[2],
            ];
        }
    }
}
