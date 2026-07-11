//! Generic pixel primitives shared by every raster: pixels, alpha
//! blending, supersampled triangles, diamond markers, rect fills,
//! compositing, and the two-facet pointer faces with their shadow rule.

pub(crate) fn set_pixel(rgba: &mut [u8], width: usize, x: i32, y: i32, rgb: [u8; 3]) {
    set_pixel_alpha(rgba, width, x, y, rgb, 255);
}

pub(crate) fn set_pixel_alpha(
    rgba: &mut [u8],
    width: usize,
    x: i32,
    y: i32,
    rgb: [u8; 3],
    alpha: u8,
) {
    if x < 0 || y < 0 || x >= width as i32 || y >= rgba.len() as i32 / 4 / width as i32 {
        return;
    }
    let i = (y as usize * width + x as usize) * 4;
    rgba[i..i + 3].copy_from_slice(&rgb);
    rgba[i + 3] = alpha;
}

pub(crate) fn draw_hud_waypoint_diamond(
    rgba: &mut [u8],
    width: usize,
    x: i32,
    y: i32,
    color: [u8; 3],
) {
    fill_diamond_rgba(rgba, width, x, y, 7.0, [18, 22, 26]);
    fill_diamond_rgba(rgba, width, x, y, 5.0, color);
}

fn fill_diamond_rgba(rgba: &mut [u8], width: usize, x: i32, y: i32, radius: f32, color: [u8; 3]) {
    let top = [x as f32, y as f32 - radius];
    let left = [x as f32 - radius, y as f32];
    let bottom = [x as f32, y as f32 + radius];
    let right = [x as f32 + radius, y as f32];
    fill_triangle_rgba(rgba, width, [top, left, bottom], color, 255);
    fill_triangle_rgba(rgba, width, [top, bottom, right], color, 255);
}

pub(crate) fn draw_diamond(rgba: &mut [u8], width: usize, x: i32, y: i32, color: [u8; 3]) {
    fill_diamond_rgba(rgba, width, x, y, 9.0, [20, 20, 20]);
    fill_diamond_rgba(rgba, width, x, y, 7.0, color);
}

pub(crate) fn fill_player_pointer_faces(
    rgba: &mut [u8],
    width: usize,
    dark: [[f32; 2]; 3],
    light: [[f32; 2]; 3],
) {
    fill_triangle_rgba(rgba, width, dark, [0xBF, 0xBF, 0xBF], 255);
    fill_triangle_rgba(rgba, width, light, [0xFF, 0xFF, 0xFF], 255);
}

pub(crate) fn player_arrow_shadow_offset() -> [i32; 2] {
    let angle = 120.0f32.to_radians();
    [
        (angle.cos() * 3.0).round() as i32,
        (angle.sin() * 3.0).round() as i32,
    ]
}

pub(crate) fn composite_alpha_mask(
    dst: &mut [u8],
    width: usize,
    mask: &[u8],
    offset: [i32; 2],
    color: [u8; 3],
    alpha: u8,
) {
    for (i, pixel) in mask.chunks_exact(4).enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let x = (i % width) as i32 + offset[0];
        let y = (i / width) as i32 + offset[1];
        let masked_alpha = ((pixel[3] as u16 * alpha as u16 + 127) / 255) as u8;
        blend_pixel(dst, width, x, y, color, masked_alpha);
    }
}

pub(crate) fn composite_rgba(dst: &mut [u8], width: usize, src: &[u8]) {
    for (i, pixel) in src.chunks_exact(4).enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        blend_pixel(
            dst,
            width,
            (i % width) as i32,
            (i / width) as i32,
            [pixel[0], pixel[1], pixel[2]],
            pixel[3],
        );
    }
}

pub(crate) fn composite_rgba_at(
    dst: &mut [u8],
    dst_width: usize,
    src: &[u8],
    src_width: usize,
    origin: [i32; 2],
) {
    for (i, pixel) in src.chunks_exact(4).enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        blend_pixel(
            dst,
            dst_width,
            origin[0] + (i % src_width) as i32,
            origin[1] + (i / src_width) as i32,
            [pixel[0], pixel[1], pixel[2]],
            pixel[3],
        );
    }
}

pub(crate) fn fill_triangle_rgba(
    rgba: &mut [u8],
    width: usize,
    points: [[f32; 2]; 3],
    color: [u8; 3],
    alpha: u8,
) {
    let min_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let max_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let min_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let max_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let edge = |a: [f32; 2], b: [f32; 2], p: [f32; 2]| {
        (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
    };
    let winding = edge(points[0], points[1], points[2]).signum();
    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let mut covered = 0u8;
            for sy in 0..4 {
                for sx in 0..4 {
                    let p = [
                        px as f32 + (sx as f32 + 0.5) * 0.25,
                        py as f32 + (sy as f32 + 0.5) * 0.25,
                    ];
                    if edge(points[0], points[1], p) * winding >= 0.0
                        && edge(points[1], points[2], p) * winding >= 0.0
                        && edge(points[2], points[0], p) * winding >= 0.0
                    {
                        covered += 1;
                    }
                }
            }
            if covered > 0 {
                blend_pixel(
                    rgba,
                    width,
                    px,
                    py,
                    color,
                    ((alpha as u16 * covered as u16 + 8) / 16) as u8,
                );
            }
        }
    }
}

pub(crate) fn blend_pixel(
    rgba: &mut [u8],
    width: usize,
    x: i32,
    y: i32,
    color: [u8; 3],
    alpha: u8,
) {
    if x < 0 || y < 0 || x >= width as i32 || y >= rgba.len() as i32 / 4 / width as i32 {
        return;
    }
    let i = (y as usize * width + x as usize) * 4;
    let src_a = alpha as f32 / 255.0;
    let dst_a = rgba[i + 3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    for channel in 0..3 {
        let src = color[channel] as f32;
        let dst = rgba[i + channel] as f32;
        rgba[i + channel] = ((src * src_a + dst * dst_a * (1.0 - src_a)) / out_a)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    rgba[i + 3] = (out_a * 255.0).round() as u8;
}

pub(crate) fn fill_rect(
    rgba: &mut [u8],
    width: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: [u8; 3],
) {
    for py in y..y + h {
        for px in x..x + w {
            set_pixel(rgba, width, px, py, color);
        }
    }
}

pub(crate) fn rects_intersect(a: [i64; 4], b: [i64; 4]) -> bool {
    a[0] < b[0] + b[2] && a[0] + a[2] > b[0] && a[1] < b[1] + b[3] && a[1] + a[3] > b[1]
}
