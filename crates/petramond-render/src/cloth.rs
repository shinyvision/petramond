//! Cloth sheets as lit, textured triangles in the item-sprite stream: one vertex per
//! simulated point, double-sided through the stream's cull-free pipeline. A painted
//! sheet is cut along its tile's texels instead, over the same surface.

use glam::{IVec3, Vec3};
use petramond_world::paint::GRID;

use crate::atlas::DYE_V_OFFSET;
use crate::item_model::ItemVertex;
use crate::lighting::{fold_tint, DynLight, LightEnv};
use crate::views::{ClothCoat, ClothPoint, ClothPresentation};

pub fn build_cloths(
    cloths: &[ClothPresentation],
    points: &[ClothPoint],
    texels: &[Option<[f32; 3]>],
    origin: IVec3,
    env: LightEnv,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    // A painted sheet samples its surface four times per texel, so its points are
    // resolved once here instead of at every sample.
    let mut surface = Vec::new();
    for cloth in cloths {
        let (cols, rows) = (usize::from(cloth.cols), usize::from(cloth.rows));
        let first = cloth.first as usize;
        let Some(grid) = points.get(first..first + cols * rows) else {
            continue;
        };
        if cols < 2 || rows < 2 {
            continue;
        }
        let offset = (cloth.cell - origin).as_vec3();
        let at = |u: usize, v: usize| grid[v * cols + u].pos;
        let point = |u: usize, v: usize| {
            let p = grid[v * cols + u];
            let du = at((u + 1).min(cols - 1), v) - at(u.saturating_sub(1), v);
            let dv = at(u, (v + 1).min(rows - 1)) - at(u, v.saturating_sub(1));
            SheetPoint {
                pos: offset + p.pos,
                shade: shade(du.cross(dv)),
                light: fold_tint([1.0; 3], DynLight::new(p.skylight, p.blocklight), env).into(),
            }
        };
        let grid_points = (0..rows).flat_map(|v| (0..cols).map(move |u| (u, v)));
        let tile = crate::atlas::tile_uv(cloth.tile);
        let flat = match cloth.coat {
            ClothCoat::Painted { first } => {
                let first = first as usize;
                let cells = usize::from(GRID) * usize::from(GRID);
                if let Some(texels) = texels.get(first..first + cells) {
                    surface.clear();
                    surface.extend(grid_points.map(|(u, v)| point(u, v)));
                    let sheet = Sheet {
                        cols,
                        rows,
                        points: &surface,
                    };
                    push_painted(&sheet, cloth.uv, tile, texels, verts, indices);
                }
                continue;
            }
            ClothCoat::Tint(tint) => Some(tint),
            ClothCoat::None => None,
        };
        let [su0, sv0, su1, sv1] = cloth.uv;
        let base = verts.len() as u32;
        verts.extend(grid_points.map(|(u, v)| {
            let fu = su0 + (su1 - su0) * u as f32 / (cols - 1) as f32;
            let fv = sv0 + (sv1 - sv0) * v as f32 / (rows - 1) as f32;
            point(u, v).vertex(tile, [fu, fv], flat)
        }));
        for v in 0..rows as u32 - 1 {
            for u in 0..cols as u32 - 1 {
                let i = base + v * cols as u32 + u;
                let below = i + cols as u32;
                indices.extend([i, i + 1, below, i + 1, below + 1, below]);
            }
        }
    }
}

/// A cloth's point grid as drawable surface points, row-major.
struct Sheet<'a> {
    cols: usize,
    rows: usize,
    points: &'a [SheetPoint],
}

#[derive(Copy, Clone)]
struct SheetPoint {
    pos: Vec3,
    shade: f32,
    light: Vec3,
}

impl SheetPoint {
    fn lerp(self, to: Self, t: f32) -> Self {
        Self {
            pos: self.pos.lerp(to.pos, t),
            shade: self.shade + (to.shade - self.shade) * t,
            light: self.light.lerp(to.light, t),
        }
    }

    /// `at` is a 0..1 position in the tile; a `dye` moves it onto the dye-base twin.
    fn vertex(self, tile: [f32; 4], at: [f32; 2], dye: Option<[f32; 3]>) -> ItemVertex {
        let [tu0, tv0, tu1, tv1] = tile;
        let twin = if dye.is_some() { DYE_V_OFFSET } else { 0.0 };
        ItemVertex {
            pos: self.pos.to_array(),
            uv: [tu0 + (tu1 - tu0) * at[0], tv0 + (tv1 - tv0) * at[1] + twin],
            shade: self.shade,
            tint: (self.light * Vec3::from(dye.unwrap_or([1.0; 3]))).to_array(),
        }
    }
}

impl Sheet<'_> {
    /// The surface at grid coordinates `(gu, gv)`, bilinear within a grid cell: every
    /// caller asking for the same coordinates gets the same point, bit for bit.
    fn sample(&self, gu: f32, gv: f32) -> SheetPoint {
        let cell = |g: f32, n: usize| (g.max(0.0) as usize).min(n - 2);
        let (u, v) = (cell(gu, self.cols), cell(gv, self.rows));
        let (tu, tv) = (gu - u as f32, gv - v as f32);
        let p = |u: usize, v: usize| self.points[v * self.cols + u];
        let top = p(u, v).lerp(p(u + 1, v), tu);
        top.lerp(p(u, v + 1).lerp(p(u + 1, v + 1), tu), tv)
    }
}

/// A painted sheet: one unshared quad per tile texel inside `uv` (texels the rect cuts
/// are clipped to it), so each can carry its own dye. Neighbouring quads sample their
/// common corners at identical coordinates, which keeps the sheet free of cracks.
fn push_painted(
    sheet: &Sheet<'_>,
    uv: [f32; 4],
    tile: [f32; 4],
    texels: &[Option<[f32; 3]>],
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    let n = f32::from(GRID);
    let [su0, sv0, su1, sv1] = uv;
    if su0 == su1 || sv0 == sv1 {
        return;
    }
    let span = |a: f32, b: f32| {
        let (lo, hi) = (a.min(b).max(0.0), a.max(b).min(1.0));
        ((lo * n).floor() as usize..(hi * n).ceil() as usize)
            .map(move |t| (t, (t as f32 / n).max(lo), ((t + 1) as f32 / n).min(hi)))
    };
    let gu = |f: f32| (f - su0) / (su1 - su0) * (sheet.cols - 1) as f32;
    let gv = |f: f32| (f - sv0) / (sv1 - sv0) * (sheet.rows - 1) as f32;
    for (ty, v0, v1) in span(sv0, sv1) {
        for (tx, u0, u1) in span(su0, su1) {
            let dye = texels[ty * usize::from(GRID) + tx];
            let base = verts.len() as u32;
            verts.extend(
                [[u0, v0], [u1, v0], [u0, v1], [u1, v1]]
                    .map(|at| sheet.sample(gu(at[0]), gv(at[1])).vertex(tile, at, dye)),
            );
            indices.extend([base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        }
    }
}

/// The block faces' directional shading, blended by the normal's direction so a sheet
/// darkens as it turns the way a cube's sides do. Both sides of a sheet shade alike.
fn shade(normal: Vec3) -> f32 {
    let n = normal.normalize_or(Vec3::Z);
    let n2 = n * n;
    n2.x * 0.6 + n2.y * 0.9 + n2.z * 0.8
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::light::BlockLight6;

    fn build(cols: u16, rows: u16, uv: [f32; 4], coat: ClothCoat) -> (Vec<ItemVertex>, Vec<u32>) {
        let points: Vec<ClothPoint> = (0..rows)
            .flat_map(|v| (0..cols).map(move |u| (f32::from(u), f32::from(v))))
            .map(|(u, v)| ClothPoint {
                pos: Vec3::new(u * 0.3, -v * 0.2, (u * 1.7 + v * 0.9).sin() * 0.1),
                skylight: 15,
                blocklight: BlockLight6::default(),
            })
            .collect();
        let cloth = ClothPresentation {
            cell: IVec3::ZERO,
            tile: petramond_world::tile::Tile::named("poppy"),
            uv,
            cols,
            rows,
            first: 0,
            coat,
        };
        let texels = [Some([1.0, 0.5, 0.25]); 256];
        let (mut verts, mut indices) = (Vec::new(), Vec::new());
        let env = LightEnv::IDENTITY;
        build_cloths(
            &[cloth],
            &points,
            &texels,
            IVec3::ZERO,
            env,
            &mut verts,
            &mut indices,
        );
        (verts, indices)
    }

    #[test]
    fn painted_sheet_has_no_cracks_and_passes_through_the_grid_points() {
        // A rect that is not texel-aligned on a coarse grid: 3 clipped texel columns
        // by 4 clipped rows.
        let uv = [0.03, 0.1, 0.17, 0.3];
        let (plain, _) = build(3, 2, uv, ClothCoat::None);
        let (painted, indices) = build(3, 2, uv, ClothCoat::Painted { first: 0 });
        let (nx, ny) = (3, 4);
        assert_eq!(painted.len(), nx * ny * 4);
        assert_eq!(indices.len(), nx * ny * 6);
        let quad = |x: usize, y: usize| &painted[(y * nx + x) * 4..][..4];
        for y in 0..ny {
            for x in 0..nx {
                let [tr, bl, br] = [1, 2, 3].map(|i| quad(x, y)[i].pos);
                if x + 1 < nx {
                    assert_eq!([tr, br], [quad(x + 1, y)[0].pos, quad(x + 1, y)[2].pos]);
                }
                if y + 1 < ny {
                    assert_eq!([bl, br], [quad(x, y + 1)[0].pos, quad(x, y + 1)[1].pos]);
                }
            }
        }
        let close = |a: [f32; 3], b: [f32; 3]| Vec3::from(a).distance(Vec3::from(b)) < 1e-5;
        assert!(close(quad(0, 0)[0].pos, plain[0].pos));
        assert!(close(quad(nx - 1, 0)[1].pos, plain[2].pos));
        assert!(close(quad(0, ny - 1)[2].pos, plain[3].pos));
        assert!(close(quad(nx - 1, ny - 1)[3].pos, plain[5].pos));
    }
}
