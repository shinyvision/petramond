//! The packed-vertex vocabulary the terrain shader shares with the mesher —
//! UV modes and face normal codes — emitted as WGSL constants from the Rust
//! definitions, so the two cannot drift.

use petramond_math::face::Face;
use std::fmt::Write;

pub(super) fn declarations() -> String {
    let mut text = String::new();
    for (name, value) in [
        ("UV_MODE_NONE", petramond_mesh::UV_MODE_NONE),
        ("UV_MODE_THIN_U", petramond_mesh::UV_MODE_THIN_U),
        ("UV_MODE_THIN_V", petramond_mesh::UV_MODE_THIN_V),
        ("UV_MODE_CELL_LOCAL", petramond_mesh::UV_MODE_CELL_LOCAL),
        // Modes from here up are transition faces; the low bits are set bits.
        ("UV_MODE_TRANSITION", petramond_mesh::UV_MODE_TRANSITION),
    ] {
        writeln!(text, "const {name}: u32 = {value}u;").unwrap();
    }
    let names = [
        "NORMAL_POS_X",
        "NORMAL_NEG_X",
        "NORMAL_POS_Y",
        "NORMAL_NEG_Y",
        "NORMAL_POS_Z",
        "NORMAL_NEG_Z",
    ];
    for (face, name) in Face::ALL.into_iter().zip(names) {
        writeln!(text, "const {name}: u32 = {}u;", face.normal_code()).unwrap();
    }
    // A cube face's shade index is a function of its normal, so a vertex that
    // spends its shade lane on other data can recover it here.
    text.push_str("fn face_shade_idx(ncode: u32) -> u32 {\n    switch ncode {\n");
    for face in Face::ALL {
        writeln!(
            text,
            "        case {}u: {{ return {}u; }}",
            face.normal_code(),
            face.shade_idx()
        )
        .unwrap();
    }
    text.push_str("        default: { return 0u; }\n    }\n}\n");
    text
}
