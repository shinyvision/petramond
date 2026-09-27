use serde::{Deserialize, Serialize};

pub use petramond_math::pose::BoxPose;

use crate::tile::Tile;

pub const CROP_PLANE_INSET: f32 = 2.0 / 16.0;

pub const CROP_PLANE_DROP: f32 = 1.0 / 16.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockLightShape {
    Open,
    OpaqueCube,
    Shaped,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShapeRenderBox {
    pub aabb: Aabb,
    pub tint: [f32; 3],
    pub ao_strength: f32,
    pub dyed: bool,
}

pub type CellPart = u8;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShapeFace {
    pub tile: Tile,
    /// Swap the cell-local u/v (the pane edge strip laid along a W/E arm).
    pub swap_uv: bool,
    /// Quarter turns (`0..4`) to rotate the cell-local UV by, applied after
    /// [`swap_uv`](Self::swap_uv).
    ///
    /// Side faces of a Y-turning shape get correct UV for free, since the turn
    /// carries their art along with them. Top/bottom faces don't: they sample a
    /// fixed tile through a rotated footprint, so without this their art stays
    /// pinned to world north. This is what lets one tile be authored once for
    /// all four facings instead of four.
    pub uv_turns: u8,
    pub tint: [f32; 3],
    pub uv_rect: Option<[u8; 4]>,
}

impl ShapeFace {
    #[inline]
    pub fn texel_uv(&self, carve: (f32, f32), fraction: (f32, f32)) -> (f32, f32) {
        let (mut u, mut v) = match self.uv_rect {
            Some([u0, v0, u1, v1]) => {
                let lerp = |a: u8, b: u8, f: f32| {
                    (f32::from(a) + (f32::from(b) - f32::from(a)) * f) / 16.0
                };
                (lerp(u0, u1, fraction.0), lerp(v0, v1, fraction.1))
            }
            None => carve,
        };
        if self.swap_uv {
            std::mem::swap(&mut u, &mut v);
        }
        Self::turn_uv(self.uv_turns, u, v)
    }

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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemBox {
    pub aabb: Aabb,
    pub faces: [bool; 6],
    pub material: Option<crate::block::Block>,
    pub tiles: [Option<crate::tile::Tile>; 6],
    pub uv_turns: [u8; 6],
    pub uv_rects: [Option<[u8; 4]>; 6],
    pub pose: Option<BoxPose>,
}

impl ItemBox {
    pub fn solid(min: [f32; 3], max: [f32; 3]) -> Self {
        ItemBox {
            aabb: Aabb { min, max },
            faces: [true; 6],
            material: None,
            tiles: [None; 6],
            uv_turns: [0; 6],
            uv_rects: [None; 6],
            pose: None,
        }
    }

    pub fn posed_bounds(&self) -> Aabb {
        posed_bounds(self.aabb, self.pose)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PosedBox {
    pub aabb: Aabb,
    pub pose: Option<BoxPose>,
}

impl PosedBox {
    pub fn bounds(&self) -> Aabb {
        posed_bounds(self.aabb, self.pose)
    }
}

pub fn posed_bounds(aabb: Aabb, pose: Option<BoxPose>) -> Aabb {
    match pose {
        Some(p) => {
            let (min, max) = p.bounds(aabb.min, aabb.max);
            Aabb { min, max }
        }
        None => aabb,
    }
}

impl Aabb {
    pub fn clipped_to_cell(&self) -> Option<Aabb> {
        let mut out = *self;
        for a in 0..3 {
            out.min[a] = out.min[a].max(0.0);
            out.max[a] = out.max[a].min(1.0);
            if out.min[a] > out.max[a] {
                return None;
            }
        }
        Some(out)
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
    pub ao_strength: f32,
    pub dyed: bool,
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
    pub casts_ao: bool,
    pub double_sided: bool,
    pub pose: Option<BoxPose>,
}

impl ShapeBox {
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
        casts_ao: true,
        double_sided: false,
        pose: None,
    };

    pub fn posed_bounds(&self) -> Aabb {
        posed_bounds(self.aabb, self.pose)
    }

    pub fn overlaps_pocket(&self, lo: [f32; 3], hi: [f32; 3]) -> bool {
        match self.pose {
            Some(p) => p.overlaps_aabb(self.aabb.min, self.aabb.max, lo, hi),
            None => (0..3).all(|a| lo[a] < self.aabb.max[a] && hi[a] > self.aabb.min[a]),
        }
    }

    pub fn uniform(aabb: Aabb, tiles: [Tile; 3], tint_for: impl Fn(Tile) -> [f32; 3]) -> Self {
        let style = |tile: Tile| {
            Some(ShapeFace {
                tile,
                swap_uv: false,
                uv_turns: 0,
                tint: tint_for(tile),
                uv_rect: None,
            })
        };
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
            casts_ao: true,
            double_sided: false,
            pose: None,
        }
    }

    pub fn with_slot_uv_turns(mut self, turns: [u8; 3]) -> Self {
        if let Some(face) = self.faces[2].as_mut() {
            face.uv_turns = turns[0];
        }
        if let Some(face) = self.faces[3].as_mut() {
            face.uv_turns = turns[1];
        }
        self
    }

    pub fn double_sided(mut self) -> Self {
        self.double_sided = true;
        self
    }

    pub fn as_face_carrier(mut self) -> Self {
        self.occludes = false;
        self
    }

    pub fn with_ao_strength(mut self, strength: f32) -> Self {
        self.ao_strength = strength;
        self
    }

    pub fn with_part(mut self, part: CellPart) -> Self {
        self.part = part;
        self
    }

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
