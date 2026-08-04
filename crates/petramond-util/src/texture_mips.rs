//! RGBA mip generation helpers for alpha-cutout pixel art.

/// Build a full mip chain for an RGBA texture. Downsampling preserves cutout coverage:
/// if any source texel in the footprint is opaque enough to survive the shader alpha
/// test, the mip texel stays fully opaque using the average opaque colour. This keeps
/// thin decals and plant-like details from disappearing at distance.
pub fn build_cutout_mips(rgba: &[u8], w: u32, h: u32) -> Vec<Vec<u8>> {
    let w = w.max(1);
    let h = h.max(1);
    let mut base = vec![0u8; (w * h * 4) as usize];
    let len = base.len().min(rgba.len());
    base[..len].copy_from_slice(&rgba[..len]);

    let mut mips = vec![base];
    let (mut src_w, mut src_h) = (w as usize, h as usize);
    while src_w > 1 || src_h > 1 {
        let dst_w = (src_w / 2).max(1);
        let dst_h = (src_h / 2).max(1);
        let src = mips.last().expect("base mip exists");
        let mut dst = vec![0u8; dst_w * dst_h * 4];

        for y in 0..dst_h {
            let y0 = y * src_h / dst_h;
            let y1 = ((y + 1) * src_h / dst_h).max(y0 + 1).min(src_h);
            for x in 0..dst_w {
                let x0 = x * src_w / dst_w;
                let x1 = ((x + 1) * src_w / dst_w).max(x0 + 1).min(src_w);
                let px = downsample_cutout_mip_pixel(src, src_w, x0, x1, y0, y1);
                let di = (y * dst_w + x) * 4;
                dst[di..di + 4].copy_from_slice(&px);
            }
        }

        mips.push(dst);
        src_w = dst_w;
        src_h = dst_h;
    }
    mips
}

fn downsample_cutout_mip_pixel(
    src: &[u8],
    src_w: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) -> [u8; 4] {
    let mut rgb = [0u32; 3];
    let mut alpha_sum = 0u32;
    let mut opaque_rgb = [0u32; 3];
    let mut opaque_count = 0u32;
    let mut sample_count = 0u32;

    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * src_w + x) * 4;
            let r = src[i] as u32;
            let g = src[i + 1] as u32;
            let b = src[i + 2] as u32;
            let a = src[i + 3] as u32;
            sample_count += 1;
            alpha_sum += a;
            if a > 0 {
                rgb[0] += r * a;
                rgb[1] += g * a;
                rgb[2] += b * a;
            }
            if a >= 128 {
                opaque_rgb[0] += r;
                opaque_rgb[1] += g;
                opaque_rgb[2] += b;
                opaque_count += 1;
            }
        }
    }

    if opaque_count > 0 {
        return [
            div_round(opaque_rgb[0], opaque_count),
            div_round(opaque_rgb[1], opaque_count),
            div_round(opaque_rgb[2], opaque_count),
            255,
        ];
    }
    if alpha_sum == 0 {
        return [0, 0, 0, 0];
    }
    [
        div_round(rgb[0], alpha_sum),
        div_round(rgb[1], alpha_sum),
        div_round(rgb[2], alpha_sum),
        div_round(alpha_sum, sample_count.max(1)),
    ]
}

#[inline]
fn div_round(n: u32, d: u32) -> u8 {
    ((n + d / 2) / d.max(1)).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_mips_preserve_opaque_coverage() {
        let rgba = [
            100, 50, 25, 255, 0, 0, 0, 0, //
            0, 0, 0, 0, 0, 0, 0, 0,
        ];

        let mips = build_cutout_mips(&rgba, 2, 2);

        assert_eq!(mips.len(), 2);
        assert_eq!(mips[1], vec![100, 50, 25, 255]);
    }

    #[test]
    fn cutout_mips_keep_empty_pixels_transparent() {
        let rgba = [0u8; 4 * 4 * 4];

        let mips = build_cutout_mips(&rgba, 4, 4);

        assert_eq!(mips.len(), 3);
        assert!(mips[1].chunks_exact(4).all(|px| px == [0, 0, 0, 0]));
        assert_eq!(mips[2], vec![0, 0, 0, 0]);
    }
}
