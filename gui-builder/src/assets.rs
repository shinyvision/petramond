//! Asset palette: the reusable parts you compose a GUI from. Builtins are drawn
//! procedurally at startup (so the tool runs with zero external files); users
//! can also import their own PNGs. Each asset keeps both its raw RGBA (for the
//! CPU bake) and an egui texture (for the live canvas + palette thumbnails).

use crate::model::{AssetSpec, LayerFit};
use eframe::egui;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Background,
    Border,
    SlotFrame,
    Highlight,
    Fill,
    Bar,
    Imported,
}

impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Background => "Backgrounds",
            AssetKind::Border => "Borders & Corners",
            AssetKind::SlotFrame => "Slot Frames",
            AssetKind::Highlight => "Highlights",
            AssetKind::Fill => "Fills",
            AssetKind::Bar => "Bars",
            AssetKind::Imported => "Imported",
        }
    }

    /// Palette display order.
    pub const ORDER: [AssetKind; 7] = [
        AssetKind::Background,
        AssetKind::Border,
        AssetKind::SlotFrame,
        AssetKind::Highlight,
        AssetKind::Bar,
        AssetKind::Fill,
        AssetKind::Imported,
    ];
}

/// A palette entry (metadata + how it should be placed when dropped).
pub struct AssetEntry {
    pub spec: AssetSpec,
    pub label: String,
    pub kind: AssetKind,
    pub default_fit: LayerFit,
    /// If true, a freshly placed layer defaults to covering the whole canvas.
    pub cover: bool,
}

/// Resolved pixels + GPU texture for one asset.
pub struct AssetData {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub tex: egui::TextureHandle,
}

pub struct AssetLibrary {
    pub entries: Vec<AssetEntry>,
    data: HashMap<AssetSpec, AssetData>,
}

impl AssetLibrary {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut lib = AssetLibrary {
            entries: Vec::new(),
            data: HashMap::new(),
        };
        for (key, kind, fit, cover, label, raster) in builtins() {
            let spec = AssetSpec::Builtin {
                key: key.to_string(),
            };
            lib.insert(
                ctx,
                spec.clone(),
                label.to_string(),
                raster,
                kind,
                fit,
                cover,
            );
        }
        lib
    }

    fn insert(
        &mut self,
        ctx: &egui::Context,
        spec: AssetSpec,
        label: String,
        raster: Raster,
        kind: AssetKind,
        default_fit: LayerFit,
        cover: bool,
    ) {
        let size = [raster.w, raster.h];
        let img = egui::ColorImage::from_rgba_unmultiplied(size, &raster.px);
        let tex = ctx.load_texture(&label, img, egui::TextureOptions::NEAREST);
        self.data.insert(
            spec.clone(),
            AssetData {
                size,
                rgba: raster.px,
                tex,
            },
        );
        self.entries.push(AssetEntry {
            spec,
            label,
            kind,
            default_fit,
            cover,
        });
    }

    pub fn get(&self, spec: &AssetSpec) -> Option<&AssetData> {
        self.data.get(spec)
    }

    pub fn entry(&self, spec: &AssetSpec) -> Option<&AssetEntry> {
        self.entries.iter().find(|e| &e.spec == spec)
    }

    /// Ensure a spec is resolved (loading file-backed assets on demand). Called
    /// after opening a `.llgui` whose layers reference imported PNGs.
    pub fn ensure(&mut self, ctx: &egui::Context, spec: &AssetSpec) -> Result<(), String> {
        if self.data.contains_key(spec) {
            return Ok(());
        }
        match spec {
            AssetSpec::Builtin { key } => Err(format!("unknown builtin asset '{key}'")),
            AssetSpec::File { path } => self.load_file(ctx, path).map(|_| ()),
        }
    }

    /// Import a PNG from disk, returning its spec on success.
    pub fn load_file(&mut self, ctx: &egui::Context, path: &Path) -> Result<AssetSpec, String> {
        let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let raster = Raster {
            w,
            h,
            px: rgba.into_raw(),
        };
        let spec = AssetSpec::File {
            path: path.to_path_buf(),
        };
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "imported".to_string());
        // Imported art: stretch by default, native size.
        self.insert(
            ctx,
            spec.clone(),
            label,
            raster,
            AssetKind::Imported,
            LayerFit::Stretch,
            false,
        );
        Ok(spec)
    }
}

// ---------------------------------------------------------------------------
// Procedural builtins
// ---------------------------------------------------------------------------

/// A simple RGBA pixel buffer used to author the builtin parts.
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Raster {
    fn new(w: usize, h: usize) -> Self {
        Raster {
            w,
            h,
            px: vec![0; w * h * 4],
        }
    }

    fn set(&mut self, x: i64, y: i64, c: [u8; 4]) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.px[i..i + 4].copy_from_slice(&c);
    }

    fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, c: [u8; 4]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, c);
            }
        }
    }

    /// Beveled border inside the given rect: light top/left, dark bottom/right.
    fn bevel(&mut self, x: i64, y: i64, w: i64, h: i64, t: i64, light: [u8; 4], dark: [u8; 4]) {
        self.fill_rect(x, y, w, t, light); // top
        self.fill_rect(x, y, t, h, light); // left
        self.fill_rect(x, y + h - t, w, t, dark); // bottom
        self.fill_rect(x + w - t, y, t, h, dark); // right
    }
}

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    [r, g, b, a]
}

/// (stable key, kind, default fit, cover, label, pixels)
fn builtins() -> Vec<(
    &'static str,
    AssetKind,
    LayerFit,
    bool,
    &'static str,
    Raster,
)> {
    vec![
        (
            "panel",
            AssetKind::Background,
            LayerFit::NineSlice {
                l: 8,
                r: 8,
                t: 8,
                b: 8,
            },
            true,
            "Panel",
            panel(),
        ),
        (
            "frame",
            AssetKind::Border,
            LayerFit::NineSlice {
                l: 5,
                r: 5,
                t: 5,
                b: 5,
            },
            true,
            "Frame",
            frame(),
        ),
        (
            "corner",
            AssetKind::Border,
            LayerFit::Stretch,
            false,
            "Corner",
            corner(),
        ),
        (
            "slot",
            AssetKind::SlotFrame,
            LayerFit::Stretch,
            false,
            "Slot",
            slot(),
        ),
        (
            "highlight",
            AssetKind::Highlight,
            LayerFit::NineSlice {
                l: 4,
                r: 4,
                t: 4,
                b: 4,
            },
            false,
            "Highlight",
            highlight(),
        ),
        (
            "bar",
            AssetKind::Bar,
            LayerFit::NineSlice {
                l: 16,
                r: 16,
                t: 4,
                b: 4,
            },
            true,
            "Bar",
            bar(),
        ),
        (
            "shell_panel",
            AssetKind::Background,
            LayerFit::NineSlice {
                l: 8,
                r: 8,
                t: 8,
                b: 8,
            },
            true,
            "Shell Panel",
            shell_panel(),
        ),
        (
            "shell_button",
            AssetKind::Bar,
            LayerFit::NineSlice {
                l: 6,
                r: 6,
                t: 6,
                b: 6,
            },
            false,
            "Shell Button",
            shell_button(),
        ),
        (
            "shell_input",
            AssetKind::Bar,
            LayerFit::NineSlice {
                l: 6,
                r: 6,
                t: 6,
                b: 6,
            },
            false,
            "Shell Input",
            shell_input(),
        ),
        (
            "shell_row",
            AssetKind::Bar,
            LayerFit::NineSlice {
                l: 4,
                r: 4,
                t: 4,
                b: 4,
            },
            false,
            "Shell Row",
            shell_row(),
        ),
        (
            "shell_scroll_track",
            AssetKind::Bar,
            LayerFit::NineSlice {
                l: 4,
                r: 4,
                t: 4,
                b: 4,
            },
            false,
            "Shell Scroll Track",
            shell_scroll_track(),
        ),
        (
            "shell_scroll_thumb",
            AssetKind::Bar,
            LayerFit::NineSlice {
                l: 4,
                r: 4,
                t: 4,
                b: 4,
            },
            false,
            "Shell Scroll Thumb",
            shell_scroll_thumb(),
        ),
        (
            "fill_light",
            AssetKind::Fill,
            LayerFit::Stretch,
            false,
            "Light",
            fill(rgba(198, 198, 198, 255)),
        ),
        (
            "fill_dark",
            AssetKind::Fill,
            LayerFit::Stretch,
            false,
            "Dark",
            fill(rgba(90, 90, 90, 255)),
        ),
    ]
}

fn panel() -> Raster {
    let mut r = Raster::new(48, 48);
    r.fill_rect(0, 0, 48, 48, rgba(198, 198, 198, 255));
    r.bevel(
        0,
        0,
        48,
        48,
        1,
        rgba(60, 60, 60, 255),
        rgba(60, 60, 60, 255),
    ); // outer dark ring
    r.bevel(
        1,
        1,
        46,
        46,
        3,
        rgba(235, 235, 235, 255),
        rgba(140, 140, 140, 255),
    ); // inner bevel
    r
}

fn frame() -> Raster {
    let mut r = Raster::new(16, 16);
    // Border only; transparent center.
    r.fill_rect(0, 0, 16, 4, rgba(0, 0, 0, 0));
    r.bevel(
        0,
        0,
        16,
        16,
        1,
        rgba(60, 60, 60, 255),
        rgba(60, 60, 60, 255),
    );
    r.bevel(
        1,
        1,
        14,
        14,
        2,
        rgba(210, 210, 210, 255),
        rgba(120, 120, 120, 255),
    );
    // punch out the center (rows/cols 4..12)
    for y in 4..12 {
        for x in 4..12 {
            r.set(x, y, rgba(0, 0, 0, 0));
        }
    }
    r
}

fn corner() -> Raster {
    let mut r = Raster::new(8, 8);
    // An L-shaped accent (top edge + left edge).
    r.fill_rect(0, 0, 8, 3, rgba(70, 70, 70, 255));
    r.fill_rect(0, 0, 3, 8, rgba(70, 70, 70, 255));
    r.fill_rect(0, 0, 7, 1, rgba(200, 200, 200, 255));
    r.fill_rect(0, 0, 1, 7, rgba(200, 200, 200, 255));
    r
}

fn slot() -> Raster {
    let mut r = Raster::new(18, 18);
    r.fill_rect(0, 0, 18, 18, rgba(139, 139, 139, 255));
    // classic inset: top/left shadow, bottom/right highlight
    r.fill_rect(0, 0, 18, 1, rgba(85, 85, 85, 255));
    r.fill_rect(0, 0, 1, 18, rgba(85, 85, 85, 255));
    r.fill_rect(0, 17, 18, 1, rgba(255, 255, 255, 255));
    r.fill_rect(17, 0, 1, 18, rgba(255, 255, 255, 255));
    r
}

/// A hover highlight: a translucent fill + brighter border, meant to be drawn
/// (nine-sliced) over a slot inflated by its margin. Mirrors the classic
/// translucent slot-hover affordance.
fn highlight() -> Raster {
    let mut r = Raster::new(16, 16);
    r.fill_rect(0, 0, 16, 16, rgba(190, 255, 235, 46));
    r.bevel(
        0,
        0,
        16,
        16,
        2,
        rgba(180, 255, 240, 120),
        rgba(180, 255, 240, 120),
    );
    r
}

fn bar() -> Raster {
    let mut r = Raster::new(182, 22);
    r.fill_rect(0, 0, 182, 22, rgba(160, 160, 160, 255));
    r.bevel(
        0,
        0,
        182,
        22,
        1,
        rgba(60, 60, 60, 255),
        rgba(60, 60, 60, 255),
    );
    r.bevel(
        1,
        1,
        180,
        20,
        2,
        rgba(220, 220, 220, 255),
        rgba(120, 120, 120, 255),
    );
    r
}

fn fill(c: [u8; 4]) -> Raster {
    let mut r = Raster::new(8, 8);
    r.fill_rect(0, 0, 8, 8, c);
    r
}

fn shell_panel() -> Raster {
    let mut r = Raster::new(32, 32);
    r.fill_rect(0, 0, 32, 32, rgba(18, 34, 37, 240));
    r.bevel(0, 0, 32, 32, 1, rgba(0, 0, 0, 255), rgba(0, 0, 0, 255));
    r.bevel(
        1,
        1,
        30,
        30,
        3,
        rgba(178, 168, 156, 255),
        rgba(50, 42, 39, 255),
    );
    r.bevel(
        4,
        4,
        24,
        24,
        1,
        rgba(12, 22, 24, 255),
        rgba(105, 96, 88, 255),
    );
    for &(x, y) in &[(2, 2), (26, 2), (2, 26), (26, 26)] {
        r.fill_rect(x, y, 4, 4, rgba(21, 18, 17, 255));
        r.fill_rect(x + 1, y + 1, 2, 2, rgba(184, 112, 45, 255));
    }
    r
}

fn shell_button() -> Raster {
    let mut r = Raster::new(32, 20);
    r.fill_rect(0, 0, 32, 20, rgba(25, 43, 46, 255));
    r.bevel(0, 0, 32, 20, 1, rgba(0, 0, 0, 255), rgba(0, 0, 0, 255));
    r.bevel(
        1,
        1,
        30,
        18,
        2,
        rgba(174, 164, 150, 255),
        rgba(47, 39, 36, 255),
    );
    r.bevel(
        3,
        3,
        26,
        14,
        1,
        rgba(9, 17, 18, 255),
        rgba(102, 94, 86, 255),
    );
    r
}

fn shell_input() -> Raster {
    let mut r = Raster::new(32, 20);
    r.fill_rect(0, 0, 32, 20, rgba(8, 15, 17, 255));
    r.bevel(0, 0, 32, 20, 1, rgba(0, 0, 0, 255), rgba(0, 0, 0, 255));
    r.bevel(
        1,
        1,
        30,
        18,
        2,
        rgba(70, 64, 59, 255),
        rgba(160, 150, 136, 255),
    );
    r.fill_rect(4, 4, 24, 12, rgba(10, 24, 27, 255));
    r
}

fn shell_row() -> Raster {
    let mut r = Raster::new(32, 16);
    r.fill_rect(0, 0, 32, 16, rgba(16, 31, 34, 230));
    r.bevel(0, 0, 32, 16, 1, rgba(0, 0, 0, 255), rgba(0, 0, 0, 255));
    r.bevel(
        1,
        1,
        30,
        14,
        1,
        rgba(120, 112, 102, 255),
        rgba(42, 35, 32, 255),
    );
    r
}

fn shell_scroll_track() -> Raster {
    let mut r = Raster::new(10, 32);
    r.fill_rect(0, 0, 10, 32, rgba(8, 15, 17, 255));
    r.bevel(0, 0, 10, 32, 1, rgba(0, 0, 0, 255), rgba(0, 0, 0, 255));
    r.bevel(
        1,
        1,
        8,
        30,
        1,
        rgba(48, 43, 39, 255),
        rgba(122, 112, 102, 255),
    );
    r.fill_rect(4, 4, 2, 24, rgba(11, 26, 29, 255));
    r
}

fn shell_scroll_thumb() -> Raster {
    let mut r = Raster::new(10, 28);
    r.fill_rect(0, 0, 10, 28, rgba(25, 43, 46, 255));
    r.bevel(0, 0, 10, 28, 1, rgba(0, 0, 0, 255), rgba(0, 0, 0, 255));
    r.bevel(
        1,
        1,
        8,
        26,
        1,
        rgba(174, 164, 150, 255),
        rgba(47, 39, 36, 255),
    );
    r.fill_rect(3, 7, 4, 1, rgba(122, 112, 102, 255));
    r.fill_rect(3, 13, 4, 1, rgba(122, 112, 102, 255));
    r.fill_rect(3, 19, 4, 1, rgba(122, 112, 102, 255));
    r
}
