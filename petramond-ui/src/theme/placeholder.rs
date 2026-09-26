//! The synthesized placeholder theme.

use super::load::parse_hex;
use super::{palette, FaceState, ImageData, Metrics, Part, PartFace, Theme};
use std::collections::BTreeMap;

impl Theme {
    /// A synthesized programmer-art theme covering every default part key:
    /// flat fills with 2px borders, distinct hues per state. Lets documents
    /// render and tests run before the real kit exists; replaced visually by
    /// the shipped `assets/ui/theme/`.
    pub fn placeholder() -> Theme {
        let mut atlas = PlaceholderAtlas::new(256, 256);
        let mut parts: BTreeMap<String, Part> = BTreeMap::new();

        let base_border = [90, 100, 110, 255];
        let single = |atlas: &mut PlaceholderAtlas,
                      parts: &mut BTreeMap<String, Part>,
                      key: &str,
                      w: u32,
                      h: u32,
                      fill: [u8; 4],
                      slice: Option<[i32; 4]>| {
            let rect = atlas.cell(w, h, fill, base_border);
            let faces = BTreeMap::from([(
                FaceState::Default,
                PartFace {
                    page: 0,
                    rect,
                    slice,
                },
            )]);
            parts.insert(
                key.to_owned(),
                Part {
                    faces,
                    label_color: None,
                    pressed_label_offset: [0, 0],
                },
            );
        };
        let multi = |atlas: &mut PlaceholderAtlas,
                     parts: &mut BTreeMap<String, Part>,
                     key: &str,
                     w: u32,
                     h: u32,
                     slice: Option<[i32; 4]>,
                     states: &[(FaceState, [u8; 4])]| {
            let mut faces = BTreeMap::new();
            for (state, fill) in states {
                let rect = atlas.cell(w, h, *fill, base_border);
                faces.insert(
                    *state,
                    PartFace {
                        page: 0,
                        rect,
                        slice,
                    },
                );
            }
            parts.insert(
                key.to_owned(),
                Part {
                    faces,
                    label_color: Some(palette::TEXT.to_owned()),
                    pressed_label_offset: [0, 1],
                },
            );
        };

        let sl4 = Some([4, 4, 4, 4]);
        single(
            &mut atlas,
            &mut parts,
            "panel.large",
            32,
            32,
            [24, 32, 40, 255],
            sl4,
        );
        single(
            &mut atlas,
            &mut parts,
            "panel.inset",
            16,
            16,
            [16, 22, 28, 255],
            sl4,
        );
        single(
            &mut atlas,
            &mut parts,
            "section.titled",
            32,
            32,
            [28, 36, 44, 255],
            Some([4, 12, 4, 4]),
        );
        multi(
            &mut atlas,
            &mut parts,
            "button.default",
            24,
            20,
            sl4,
            &[
                (FaceState::Default, [45, 58, 70, 255]),
                (FaceState::Hover, [62, 80, 96, 255]),
                (FaceState::Pressed, [35, 45, 55, 255]),
                (FaceState::Disabled, [38, 42, 46, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "button.success",
            24,
            20,
            sl4,
            &[
                (FaceState::Default, [40, 90, 45, 255]),
                (FaceState::Hover, [55, 115, 60, 255]),
                (FaceState::Pressed, [30, 70, 35, 255]),
                (FaceState::Disabled, [40, 52, 42, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "button.danger",
            24,
            20,
            sl4,
            &[
                (FaceState::Default, [110, 40, 40, 255]),
                (FaceState::Hover, [140, 55, 55, 255]),
                (FaceState::Pressed, [85, 30, 30, 255]),
                (FaceState::Disabled, [56, 40, 40, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "checkbox",
            10,
            10,
            None,
            &[
                (FaceState::Off, [30, 38, 46, 255]),
                (FaceState::On, [80, 190, 90, 255]),
                (FaceState::Disabled, [40, 44, 48, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "toggle",
            18,
            10,
            None,
            &[
                (FaceState::Off, [55, 60, 66, 255]),
                (FaceState::On, [70, 170, 80, 255]),
                (FaceState::Disabled, [42, 46, 50, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "slot",
            18,
            18,
            Some([1, 1, 1, 1]),
            &[
                (FaceState::Default, [20, 26, 32, 255]),
                (FaceState::Hover, [90, 110, 130, 160]),
            ],
        );
        single(
            &mut atlas,
            &mut parts,
            "scrollbar.track",
            8,
            24,
            [18, 24, 30, 255],
            Some([2, 2, 2, 2]),
        );
        multi(
            &mut atlas,
            &mut parts,
            "scrollbar.thumb",
            8,
            16,
            Some([2, 2, 2, 2]),
            &[
                (FaceState::Default, [90, 100, 110, 255]),
                (FaceState::Hover, [120, 132, 144, 255]),
            ],
        );
        single(
            &mut atlas,
            &mut parts,
            "slider.track",
            24,
            6,
            [30, 60, 90, 255],
            Some([2, 2, 2, 2]),
        );
        multi(
            &mut atlas,
            &mut parts,
            "slider.handle",
            8,
            14,
            None,
            &[
                (FaceState::Default, [150, 160, 170, 255]),
                (FaceState::Hover, [190, 200, 210, 255]),
                (FaceState::Pressed, [120, 130, 140, 255]),
                (FaceState::Disabled, [80, 84, 88, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "list.row",
            32,
            26,
            sl4,
            &[
                (FaceState::Default, [26, 34, 42, 255]),
                (FaceState::Hover, [38, 50, 62, 255]),
                (FaceState::Selected, [50, 70, 100, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "tab",
            24,
            20,
            sl4,
            &[
                (FaceState::Default, [34, 44, 54, 255]),
                (FaceState::Hover, [50, 64, 78, 255]),
                (FaceState::Selected, [24, 32, 40, 255]),
                (FaceState::Disabled, [36, 40, 44, 255]),
            ],
        );
        multi(
            &mut atlas,
            &mut parts,
            "input",
            32,
            18,
            sl4,
            &[
                (FaceState::Default, [16, 20, 24, 255]),
                (FaceState::Focus, [22, 30, 40, 255]),
                (FaceState::Disabled, [30, 32, 34, 255]),
            ],
        );
        single(
            &mut atlas,
            &mut parts,
            "badge",
            16,
            13,
            [40, 52, 64, 255],
            Some([2, 2, 2, 2]),
        );
        for (level, fill) in [
            ("info", [30, 60, 110, 255]),
            ("warning", [110, 90, 20, 255]),
            ("success", [30, 90, 40, 255]),
            ("danger", [110, 35, 35, 255]),
        ] {
            single(
                &mut atlas,
                &mut parts,
                &format!("alert.{level}"),
                32,
                20,
                fill,
                sl4,
            );
        }
        multi(
            &mut atlas,
            &mut parts,
            "gauge.arrow",
            24,
            17,
            None,
            &[(FaceState::Empty, [40, 44, 48, 255]), (FaceState::Full, [230, 230, 230, 255])],
        );
        multi(
            &mut atlas,
            &mut parts,
            "gauge.flame",
            14,
            14,
            None,
            &[(FaceState::Empty, [40, 40, 40, 255]), (FaceState::Full, [230, 140, 40, 255])],
        );
        single(&mut atlas, &mut parts, "label", 1, 1, [0, 0, 0, 0], None);

        let mut palette = BTreeMap::new();
        for (k, v) in [
            ("text", "#E8EDF2"),
            ("text_muted", "#9AA7B4"),
            ("text_disabled", "#5E6B78"),
            ("accent", "#57C956"),
            ("danger", "#E4574F"),
            ("selection", "#3E6FD9"),
            ("dim", "#00000080"),
        ] {
            palette.insert(k.to_owned(), parse_hex(v).expect("placeholder colours parse"));
        }

        Theme {
            palette,
            parts,
            metrics: Metrics::default(),
            pages: vec![atlas.finish()],
            ui_font: std::sync::Arc::new(crate::text::Font::builtin()),
        }
    }
}

/// Shelf-packs flat-colored bordered cells into an RGBA atlas.
struct PlaceholderAtlas {
    rgba: Vec<u8>,
    size: (u32, u32),
    cursor: (u32, u32),
    row_h: u32,
}

impl PlaceholderAtlas {
    fn new(w: u32, h: u32) -> PlaceholderAtlas {
        PlaceholderAtlas {
            rgba: vec![0; (w * h * 4) as usize],
            size: (w, h),
            cursor: (0, 0),
            row_h: 0,
        }
    }

    fn cell(&mut self, w: u32, h: u32, fill: [u8; 4], border: [u8; 4]) -> [u32; 4] {
        if self.cursor.0 + w > self.size.0 {
            self.cursor = (0, self.cursor.1 + self.row_h + 1);
            self.row_h = 0;
        }
        assert!(
            self.cursor.1 + h <= self.size.1,
            "placeholder atlas overflow"
        );
        let (x0, y0) = self.cursor;
        for y in 0..h {
            for x in 0..w {
                let on_border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                let c = if on_border { border } else { fill };
                let i = (((y0 + y) * self.size.0 + x0 + x) * 4) as usize;
                self.rgba[i..i + 4].copy_from_slice(&c);
            }
        }
        self.cursor.0 += w + 1;
        self.row_h = self.row_h.max(h);
        [x0, y0, w, h]
    }

    fn finish(self) -> ImageData {
        ImageData {
            rgba: self.rgba,
            size: self.size,
        }
    }
}
