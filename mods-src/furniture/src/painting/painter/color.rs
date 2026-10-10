//! The picker's colour: hue, saturation and value, each `0..=1`.

use crate::painting::design::Rgb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    pub fn to_rgb(self) -> Rgb {
        let sector = self.h.rem_euclid(1.0) * 6.0;
        let chroma = self.v * self.s;
        let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
        let (r, g, b) = match sector as u8 {
            0 => (chroma, second, 0.0),
            1 => (second, chroma, 0.0),
            2 => (0.0, chroma, second),
            3 => (0.0, second, chroma),
            4 => (second, 0.0, chroma),
            _ => (chroma, 0.0, second),
        };
        let floor = self.v - chroma;
        [r, g, b].map(|c| ((c + floor) * 255.0).round().clamp(0.0, 255.0) as u8)
    }

    /// A grey has no hue of its own; it keeps `hue`, so picking one does not swing the picker.
    pub fn from_rgb(rgb: Rgb, hue: f32) -> Hsv {
        let [r, g, b] = rgb.map(|c| f32::from(c) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let chroma = max - min;
        if chroma <= 0.0 {
            return Hsv {
                h: hue,
                s: 0.0,
                v: max,
            };
        }
        let sector = if max == r {
            ((g - b) / chroma).rem_euclid(6.0)
        } else if max == g {
            (b - r) / chroma + 2.0
        } else {
            (r - g) / chroma + 4.0
        };
        Hsv {
            h: sector / 6.0,
            s: chroma / max,
            v: max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_picked_colour_reads_back_as_the_colour_it_shows() {
        for rgb in [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [250, 224, 28],
            [40, 84, 232],
            [13, 200, 199],
            [128, 128, 128],
            [0, 0, 0],
            [255, 255, 255],
        ] {
            assert_eq!(Hsv::from_rgb(rgb, 0.3).to_rgb(), rgb);
        }
    }

    #[test]
    fn a_grey_keeps_the_hue_the_picker_was_on() {
        assert_eq!(Hsv::from_rgb([90, 90, 90], 0.7).h, 0.7);
    }
}
