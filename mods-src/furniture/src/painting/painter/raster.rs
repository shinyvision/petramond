//! The table's pictures, and the way back from a spot on one to what it stands for. Each is
//! drawn at the size its widget is laid out at, so a pointer position is a pixel of it.

use super::color::Hsv;
use crate::painting::design::{Design, Rgb};

pub(super) struct Picture {
    pub w: u16,
    pub h: u16,
    pub rgba: Vec<u8>,
}

impl Picture {
    fn new(w: u16, h: u16) -> Picture {
        Picture {
            w,
            h,
            rgba: vec![0; usize::from(w) * usize::from(h) * 4],
        }
    }

    fn put(&mut self, x: i32, y: i32, [r, g, b]: Rgb) {
        if (0..i32::from(self.w)).contains(&x) && (0..i32::from(self.h)).contains(&y) {
            let i = (y as usize * usize::from(self.w) + x as usize) * 4;
            self.rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
}

/// What the canvas shows with no flag on the table: nothing.
pub(super) fn blank() -> Picture {
    Picture::new(1, 1)
}

const BLACK: Rgb = [0; 3];
const WHITE: Rgb = [255; 3];

/// Screen pixels per texel of the design.
pub(super) const CANVAS_ZOOM: u16 = 8;
pub(super) const WHEEL: u16 = 64;
pub(super) const BAR: [u16; 2] = [64, 8];
/// Swatches per row, and their rows.
pub(super) const SWATCH_GRID: [u16; 2] = [8, 2];
/// A swatch's side.
const SWATCH: u16 = 8;

/// The design, enlarged, each texel's far edges nudged toward grey so the grid shows on any
/// colour.
pub(super) fn canvas(design: &Design) -> Picture {
    let [w, h] = design.size().map(u16::from);
    let mut picture = Picture::new(w * CANVAS_ZOOM, h * CANVAS_ZOOM);
    for (ty, row) in design.rows().enumerate() {
        for (tx, &color) in row.iter().enumerate() {
            let edge = color.map(|c| (i32::from(c) + (128 - i32::from(c)) / 6) as u8);
            for py in 0..CANVAS_ZOOM {
                for px in 0..CANVAS_ZOOM {
                    let on_edge = px == CANVAS_ZOOM - 1 || py == CANVAS_ZOOM - 1;
                    picture.put(
                        (tx as u16 * CANVAS_ZOOM + px).into(),
                        (ty as u16 * CANVAS_ZOOM + py).into(),
                        if on_edge { edge } else { color },
                    );
                }
            }
        }
    }
    picture
}

/// The texel under a spot on the canvas picture; it may lie off the design.
pub(super) fn canvas_texel(x: f32, y: f32) -> [i32; 2] {
    [x, y].map(|v| (v / f32::from(CANVAS_ZOOM)).floor() as i32)
}

/// Hue around the disc and saturation out from its middle, at the colour's own value.
pub(super) fn wheel(color: Hsv) -> Picture {
    let mut picture = Picture::new(WHEEL, WHEEL);
    for y in 0..WHEEL {
        for x in 0..WHEEL {
            let at = wheel_pick(f32::from(x) + 0.5, f32::from(y) + 0.5, color.v);
            if at.s <= 1.0 {
                picture.put(x.into(), y.into(), at.to_rgb());
            }
        }
    }
    let radius = f32::from(WHEEL) / 2.0;
    let angle = color.h * std::f32::consts::TAU;
    let reach = color.s * (radius - 1.0);
    ring(
        &mut picture,
        (radius + angle.cos() * reach) as i32,
        (radius + angle.sin() * reach) as i32,
    );
    picture
}

/// The colour a spot on the wheel stands for; its saturation passes 1 beyond the rim.
pub(super) fn wheel_pick(x: f32, y: f32, value: f32) -> Hsv {
    let radius = f32::from(WHEEL) / 2.0;
    let (dx, dy) = (x - radius, y - radius);
    Hsv {
        h: (dy.atan2(dx) / std::f32::consts::TAU).rem_euclid(1.0),
        s: dx.hypot(dy) / radius,
        v: value,
    }
}

/// Every hue at full strength, left to right.
pub(super) fn hue_bar(color: Hsv) -> Picture {
    bar(color.h, |h| Hsv { h, s: 1.0, v: 1.0 })
}

/// The colour from black up to its brightest, left to right.
pub(super) fn value_bar(color: Hsv) -> Picture {
    bar(color.v, |v| Hsv { v, ..color })
}

fn bar(marked: f32, at: impl Fn(f32) -> Hsv) -> Picture {
    let [w, h] = BAR;
    let mut picture = Picture::new(w, h);
    for x in 0..w {
        let color = at(bar_pick(f32::from(x) + 0.5)).to_rgb();
        for y in 0..h {
            picture.put(x.into(), y.into(), color);
        }
    }
    let x = (marked * f32::from(w - 1)).round() as i32;
    for y in 0..i32::from(h) {
        picture.put(x - 1, y, BLACK);
        picture.put(x, y, WHITE);
        picture.put(x + 1, y, BLACK);
    }
    picture
}

/// How far along a bar a spot is, `0..=1`; a drag past either end stays on it.
pub(super) fn bar_pick(x: f32) -> f32 {
    (x / f32::from(BAR[0])).clamp(0.0, 1.0)
}

pub(super) fn swatches(colors: &[Rgb]) -> Picture {
    let [cols, rows] = SWATCH_GRID;
    let mut picture = Picture::new(cols * SWATCH, rows * SWATCH);
    for (i, &color) in colors.iter().take(usize::from(cols * rows)).enumerate() {
        let (col, row) = (i as u16 % cols, i as u16 / cols);
        for y in 0..SWATCH {
            for x in 0..SWATCH {
                let rim = x == 0 || y == 0 || x == SWATCH - 1 || y == SWATCH - 1;
                picture.put(
                    (col * SWATCH + x).into(),
                    (row * SWATCH + y).into(),
                    if rim { BLACK } else { color },
                );
            }
        }
    }
    picture
}

/// Which swatch a spot is on.
pub(super) fn swatch_at(x: f32, y: f32) -> Option<usize> {
    let [cols, rows] = SWATCH_GRID.map(f32::from);
    let [col, row] = [x, y].map(|v| (v / f32::from(SWATCH)).floor());
    let on_swatch = (0.0..cols).contains(&col) && (0.0..rows).contains(&row);
    on_swatch.then_some((row * cols + col) as usize)
}

/// A small white ring in a black one, readable over any colour.
fn ring(picture: &mut Picture, cx: i32, cy: i32) {
    for dy in -3..=3i32 {
        for dx in -3..=3i32 {
            match dx * dx + dy * dy {
                3..=6 => picture.put(cx + dx, cy + dy, WHITE),
                7..=10 => picture.put(cx + dx, cy + dy, BLACK),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painting::design::Canvas;

    fn pixel(picture: &Picture, x: u16, y: u16) -> [u8; 4] {
        let i = (usize::from(y) * usize::from(picture.w) + usize::from(x)) * 4;
        [0, 1, 2, 3].map(|c| picture.rgba[i + c])
    }

    #[test]
    fn a_spot_on_the_canvas_picture_is_the_texel_drawn_there() {
        let rect = Canvas::new([0, 2, 16, 12]).expect("a rect inside the tile");
        let mut design = Design::filled(rect, [255; 3]);
        design.set([5, 7], [10, 200, 30]);
        let picture = canvas(&design);
        assert_eq!([picture.w, picture.h], [16 * CANVAS_ZOOM, 12 * CANVAS_ZOOM]);
        let (x, y) = (5 * CANVAS_ZOOM + 2, 7 * CANVAS_ZOOM + 2);
        assert_eq!(pixel(&picture, x, y), [10, 200, 30, 255]);
        assert_eq!(canvas_texel(f32::from(x) + 0.5, f32::from(y) + 0.5), [5, 7]);
        assert_eq!(
            canvas_texel(-0.5, 3.0),
            [-1, 0],
            "left of the canvas is off it"
        );
    }

    #[test]
    fn a_spot_on_the_wheel_is_the_colour_drawn_there_and_the_corners_are_clear() {
        let color = Hsv {
            h: 0.0,
            s: 0.0,
            v: 0.8,
        };
        let picture = wheel(color);
        assert_eq!(pixel(&picture, 0, 0)[3], 0);
        let (x, y) = (50, 20);
        let picked = wheel_pick(f32::from(x) + 0.5, f32::from(y) + 0.5, color.v).to_rgb();
        assert_eq!(pixel(&picture, x, y)[..3], picked);
    }

    #[test]
    fn a_spot_on_a_swatch_names_it_and_a_spot_past_them_names_none() {
        let colors: Vec<Rgb> = (0..16).map(|i| [i * 10, 0, 0]).collect();
        let picture = swatches(&colors);
        assert_eq!(pixel(&picture, 8 * 3 + 4, 8 + 4)[..3], colors[11]);
        assert_eq!(swatch_at(8.0 * 3.0 + 4.0, 8.0 + 4.0), Some(11));
        assert_eq!(swatch_at(70.0, 2.0), None);
        assert_eq!(swatch_at(4.0, -1.0), None);
    }
}
