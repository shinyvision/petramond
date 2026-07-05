//! The combined model texture atlas — every kind's texture stacked into one sheet with
//! a per-kind UV transform — plus the pre-scanned break/mining particle patches
//! sampled from it.

use std::sync::LazyLock;

use super::{all, BlockModelKind, MODELS};

/// Every model kind's texture packed into ONE RGBA sheet (vertically stacked), with a
/// per-kind UV transform into it, so all model geometry in a chunk draws with a single
/// texture bind. Built once from [`MODELS`]; the mesher remaps each face UV through
/// [`remap`](Self::remap) and the renderer uploads [`rgba`](Self::rgba).
pub struct ModelAtlas {
    rgba: Vec<u8>,
    w: u32,
    h: u32,
    /// Per-kind `[u_off, v_off, u_scale, v_scale]` mapping the kind's own `[0,1]` UVs
    /// into the combined sheet.
    xform: Vec<[f32; 4]>,
}

impl ModelAtlas {
    fn build() -> Self {
        // Vertical stack: width = widest texture, height = sum of heights. No rectangle
        // packing needed and it generalises to any number of kinds.
        let w = MODELS.iter().map(|m| m.tex_w).max().unwrap_or(1).max(1);
        let total_h: u32 = MODELS.iter().map(|m| m.tex_h).sum::<u32>().max(1);
        let mut rgba = vec![0u8; (w * total_h * 4) as usize];
        let mut xform = Vec::with_capacity(MODELS.len());
        let mut y_off = 0u32;
        for m in MODELS.iter() {
            // Blit this model's texture into the sheet at (0, y_off).
            for row in 0..m.tex_h {
                let src = (row * m.tex_w * 4) as usize;
                let dst = ((y_off + row) * w * 4) as usize;
                let n = (m.tex_w * 4) as usize;
                if src + n <= m.texture_rgba.len() && dst + n <= rgba.len() {
                    rgba[dst..dst + n].copy_from_slice(&m.texture_rgba[src..src + n]);
                }
            }
            xform.push([
                0.0,
                y_off as f32 / total_h as f32,
                m.tex_w as f32 / w as f32,
                m.tex_h as f32 / total_h as f32,
            ]);
            y_off += m.tex_h;
        }
        ModelAtlas {
            rgba,
            w,
            h: total_h,
            xform,
        }
    }

    /// The combined sheet bytes + dimensions, for GPU upload.
    pub fn texture(&self) -> (&[u8], u32, u32) {
        (&self.rgba, self.w, self.h)
    }

    /// Remap a model-local `[u, v]` (in `kind`'s own `[0,1]` texture) into the combined
    /// sheet's UV space.
    #[inline]
    pub fn remap(&self, kind: BlockModelKind, uv: [f32; 2]) -> [f32; 2] {
        let [uo, vo, us, vs] = self.xform[kind.0 as usize];
        [uo + uv[0] * us, vo + uv[1] * vs]
    }

    /// The alpha byte (`0..=255`) of the combined sheet at normalized `uv` (nearest
    /// texel; UV clamped to the edge) — the texel opacity the pixel-perfect ray pick
    /// ([`ray_vs_model`]) tests so a hit only lands on a non-transparent texel.
    #[inline]
    pub fn alpha_at(&self, uv: [f32; 2]) -> u8 {
        let x = ((uv[0] * self.w as f32) as i32).clamp(0, self.w as i32 - 1) as u32;
        let y = ((uv[1] * self.h as f32) as i32).clamp(0, self.h as i32 - 1) as u32;
        let idx = ((y * self.w + x) * 4 + 3) as usize;
        self.rgba.get(idx).copied().unwrap_or(255)
    }
}

/// The combined model texture atlas (built once).
pub fn atlas() -> &'static ModelAtlas {
    static ATLAS: LazyLock<ModelAtlas> = LazyLock::new(ModelAtlas::build);
    &ATLAS
}

// ---------------------------------------------------------------------------------
// Break/mining particle texture patches
// ---------------------------------------------------------------------------------

/// Pre-scanned OPAQUE fleck patches for a kind: model-local `[u, v]` mins of small
/// square texture patches whose centre texel is opaque, plus the patch edge in
/// model-local UV. So break/mining flecks sample the model's OWN texture (wood grain,
/// not the crafting-table placeholder) and almost never land on a fully transparent
/// patch (which would render as an invisible fleck).
struct ParticlePatches {
    mins: Vec<[f32; 2]>,
    size_local: f32,
}

static PATCHES: LazyLock<Vec<ParticlePatches>> =
    LazyLock::new(|| all().iter().map(|&k| ParticlePatches::scan(k)).collect());

impl ParticlePatches {
    fn scan(kind: BlockModelKind) -> Self {
        let m = &MODELS[kind.0 as usize];
        let (tw, th) = (m.tex_w.max(1), m.tex_h.max(1));
        // A 4-texel fleck patch, stepped across the sheet on the same stride.
        let patch = 4u32.min(tw).min(th);
        let mut mins = Vec::new();
        let mut y = 0;
        while y + patch <= th {
            let mut x = 0;
            while x + patch <= tw {
                let (cx, cy) = (x + patch / 2, y + patch / 2);
                let idx = ((cy * tw + cx) * 4 + 3) as usize;
                if m.texture_rgba.get(idx).copied().unwrap_or(0) >= 128 {
                    mins.push([x as f32 / tw as f32, y as f32 / th as f32]);
                }
                x += patch;
            }
            y += patch;
        }
        ParticlePatches {
            mins,
            size_local: patch as f32 / tw as f32,
        }
    }
}

/// An ABSOLUTE model-atlas UV patch (`min`, square `size`) for one break/mining fleck of
/// `kind`, chosen from its opaque texture patches by `r` (`0..1`). So a model block's
/// flecks read as its own texture; falls back to the whole sheet if nothing scanned
/// opaque. Shared by [`crate::entity::ParticleSystem`]'s model spawn paths.
pub fn particle_patch(kind: BlockModelKind, r: f32) -> ([f32; 2], f32) {
    let p = &PATCHES[kind.0 as usize];
    let at = atlas();
    let min_local = if p.mins.is_empty() {
        [0.0, 0.0]
    } else {
        let i = ((r.clamp(0.0, 1.0) * p.mins.len() as f32) as usize).min(p.mins.len() - 1);
        p.mins[i]
    };
    let size = if p.mins.is_empty() { 1.0 } else { p.size_local };
    let amin = at.remap(kind, min_local);
    let amax = at.remap(kind, [min_local[0] + size, min_local[1] + size]);
    (amin, (amax[0] - amin[0]).max(1e-4))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WB: BlockModelKind = BlockModelKind::FurnitureWorkbench;

    #[test]
    fn atlas_remap_is_within_unit_square() {
        let at = atlas();
        let (_, w, h) = at.texture();
        assert!(w >= 1 && h >= 1);
        for &uv in &[[0.0, 0.0], [1.0, 1.0], [0.5, 0.25]] {
            let [u, v] = at.remap(WB, uv);
            assert!((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v));
        }
    }
}
