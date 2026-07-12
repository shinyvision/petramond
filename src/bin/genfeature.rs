//! Headless feature previewer (dev tool).
//!
//! Renders one configured terrain feature into PNGs without terrain or chunks, so
//! feature shape can be reviewed directly.
//!
//! Run:
//!   cargo run --quiet --bin genfeature -- [feature] [out.png] [seed] [view] [scale]
//! e.g.
//!   cargo run --quiet --bin genfeature -- redwood /tmp/redwood.png 42 side 8
//!   cargo run --quiet --bin genfeature -- redwood /tmp/redwood.png 42 all 8

use petramond::tooling::block::Block;
use petramond::tooling::worldgen::{feature_preview_names, preview_feature, FeaturePreview};

#[derive(Copy, Clone)]
enum View {
    Side,
    Front,
    Top,
}

impl View {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "side" => Some(Self::Side),
            "front" => Some(Self::Front),
            "top" => Some(Self::Top),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Side => "side",
            Self::Front => "front",
            Self::Top => "top",
        }
    }

    fn project(self, pos: [i32; 3]) -> (i32, i32, i32) {
        match self {
            Self::Side => (pos[0], pos[1], pos[2]),
            Self::Front => (pos[2], pos[1], pos[0]),
            Self::Top => (pos[0], pos[2], pos[1]),
        }
    }
}

#[derive(Copy, Clone)]
struct Cell {
    priority: u8,
    depth: i32,
    block: Block,
}

fn block_color(block: Block) -> [u8; 3] {
    match block {
        Block::OakLog => [104, 74, 40],
        Block::SpruceLog => [70, 50, 32],
        Block::BirchLog => [206, 206, 198],
        Block::JungleLog | Block::AcaciaLog => [120, 90, 54],
        Block::RedwoodLog => [127, 60, 51],
        Block::OakLeaves => [44, 110, 40],
        Block::SpruceLeaves => [40, 78, 52],
        Block::BirchLeaves => [108, 150, 78],
        Block::JungleLeaves => [48, 124, 28],
        Block::AcaciaLeaves => [104, 130, 40],
        Block::RedwoodLeaves => [38, 86, 46],
        _ => [120, 120, 124],
    }
}

fn priority(block: Block) -> u8 {
    if block.is_log() {
        3
    } else if block.is_leaves() {
        2
    } else {
        1
    }
}

fn shade(mut color: [u8; 3], factor: f32) -> [u8; 3] {
    for c in &mut color {
        *c = (*c as f32 * factor).clamp(0.0, 255.0) as u8;
    }
    color
}

fn render(preview: &FeaturePreview, view: View, out: &str, scale: usize) {
    let scale = scale.max(1);
    let pad = 2i32;
    let mut min_u = i32::MAX;
    let mut max_u = i32::MIN;
    let mut min_v = i32::MAX;
    let mut max_v = i32::MIN;
    let mut min_d = i32::MAX;
    let mut max_d = i32::MIN;

    for voxel in &preview.voxels {
        let (u, v, d) = view.project(voxel.pos);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
        min_d = min_d.min(d);
        max_d = max_d.max(d);
    }

    let cells_w = (max_u - min_u + 1 + pad * 2).max(1) as usize;
    let cells_h = (max_v - min_v + 1 + pad * 2).max(1) as usize;
    let mut cells: Vec<Option<Cell>> = vec![None; cells_w * cells_h];

    for voxel in &preview.voxels {
        let (u, v, d) = view.project(voxel.pos);
        let x = (u - min_u + pad) as usize;
        let y = (max_v - v + pad) as usize;
        let idx = y * cells_w + x;
        let next = Cell {
            priority: priority(voxel.block),
            depth: d,
            block: voxel.block,
        };
        let replace = match cells[idx] {
            Some(cur) => {
                next.priority > cur.priority
                    || (next.priority == cur.priority && next.depth >= cur.depth)
            }
            None => true,
        };
        if replace {
            cells[idx] = Some(next);
        }
    }

    let w = cells_w * scale;
    let h = cells_h * scale;
    let mut buf = vec![0u8; w * h * 3];
    for px in buf.chunks_mut(3) {
        px.copy_from_slice(&[150, 190, 232]);
    }

    let depth_span = (max_d - min_d).max(1) as f32;
    for cy in 0..cells_h {
        for cx in 0..cells_w {
            let Some(cell) = cells[cy * cells_w + cx] else {
                continue;
            };
            let t = (cell.depth - min_d) as f32 / depth_span;
            let color = shade(block_color(cell.block), 0.72 + 0.36 * t);
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = (cy * scale + dy) * w + (cx * scale + dx);
                    buf[px * 3..px * 3 + 3].copy_from_slice(&color);
                }
            }
        }
    }

    image::save_buffer(out, &buf, w as u32, h as u32, image::ColorType::Rgb8)
        .expect("write feature png");
    println!(
        "wrote {out} ({w}x{h}, view {}, blocks {})",
        view.name(),
        preview.voxels.len()
    );
}

fn parse_seed(s: &str) -> Option<u32> {
    if let Some(hex) = s.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn with_suffix(out: &str, suffix: &str) -> String {
    match out.rfind('.') {
        Some(dot) => format!("{}_{}{}", &out[..dot], suffix, &out[dot..]),
        None => format!("{out}_{suffix}.png"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let feature = args.next().unwrap_or_else(|| "redwood".to_string());
    if feature == "list" {
        println!("{}", feature_preview_names().join("\n"));
        return;
    }

    let out = args
        .next()
        .unwrap_or_else(|| format!("/tmp/{feature}_feature.png"));
    let seed = args.next().as_deref().and_then(parse_seed).unwrap_or(42);
    let view = args.next().unwrap_or_else(|| "side".to_string());
    let scale = args
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(8);

    let Some(preview) = preview_feature(&feature, seed) else {
        eprintln!(
            "unknown feature '{feature}'. Known features:\n{}",
            feature_preview_names().join("\n")
        );
        std::process::exit(2);
    };

    let b = preview.bounds;
    println!(
        "feature {feature} seed {seed:#x} bounds min={:?} max={:?}",
        b.min, b.max
    );

    if view == "all" {
        for v in [View::Side, View::Front, View::Top] {
            render(&preview, v, &with_suffix(&out, v.name()), scale);
        }
    } else if let Some(v) = View::parse(&view) {
        render(&preview, v, &out, scale);
    } else {
        eprintln!("unknown view '{view}'. Use side, front, top, or all.");
        std::process::exit(2);
    }
}
