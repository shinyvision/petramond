use std::fmt::Write;

use super::transition::UV_MODE_TRANSITION;
use super::{
    AO_SHIFT, BLOCK_LIGHT_MASK, CELL_UV_MASK, CELL_UV_U_SHIFT, CELL_UV_V_SHIFT, CHROMA_HI_MASK,
    CHROMA_HI_SHIFT, CHROMA_LO_BITS, CORNER_SHIFT, DYED_FLAG2, FLUID_FLOW_FLAG2, FLUID_MEDIUM_MASK,
    FLUID_MEDIUM_SHIFT, NORMAL_CODE_MASK, NORMAL_CODE_SHIFT, OVERLAY_FLAG, OVERLAY_MASK,
    OVERLAY_SHIFT2, SHADE_SHIFT, SKY_SHIFT, TILE_MASK, UV_MODE_CELL_LOCAL, UV_MODE_NONE,
    UV_MODE_SHIFT, UV_MODE_THIN_U, UV_MODE_THIN_V, UV_TURN_HI_FLAG2, UV_TURN_LO_FLAG,
};
use crate::face::{Face, FaceShading};

const TWO_BITS: u32 = 0x3;
const LIGHT_BITS: u32 = 0x3F;
const UV_MODE_MASK: u32 = 0x7;
const SPAN_NIBBLE: u32 = 0xF;

pub fn layout() -> String {
    let mut text = String::from(
        "// petramond::vertex — generated from petramond_mesh::vertex; do not copy.\n",
    );
    text.push_str(&vocabulary());
    text.push_str(&lane_decoders());
    text.push_str(&super::transition::wgsl());
    text
}

fn vocabulary() -> String {
    let mut text = String::new();
    for (name, value) in [
        ("UV_MODE_NONE", UV_MODE_NONE),
        ("UV_MODE_THIN_U", UV_MODE_THIN_U),
        ("UV_MODE_THIN_V", UV_MODE_THIN_V),
        ("UV_MODE_CELL_LOCAL", UV_MODE_CELL_LOCAL),
        ("UV_MODE_TRANSITION", UV_MODE_TRANSITION),
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

fn lane(text: &mut String, name: &str, word: &str, shift: u32, mask: u32) {
    let written = if shift == 0 {
        writeln!(
            text,
            "fn {name}({word}: u32) -> u32 {{ return {word} & {mask:#X}u; }}"
        )
    } else {
        writeln!(
            text,
            "fn {name}({word}: u32) -> u32 {{ return ({word} >> {shift}u) & {mask:#X}u; }}"
        )
    };
    written.unwrap();
}

fn lane_decoders() -> String {
    let mut text = String::new();
    lane(&mut text, "vtx_tile", "packed", 0, TILE_MASK);
    lane(&mut text, "vtx_corner", "packed", CORNER_SHIFT, TWO_BITS);
    lane(&mut text, "vtx_shade", "packed", SHADE_SHIFT, TWO_BITS);
    lane(&mut text, "vtx_ao", "packed", AO_SHIFT, TWO_BITS);
    lane(&mut text, "vtx_sky", "packed", SKY_SHIFT, LIGHT_BITS);
    lane(
        &mut text,
        "vtx_uv_mode",
        "packed",
        UV_MODE_SHIFT,
        UV_MODE_MASK,
    );
    lane(
        &mut text,
        "vtx_overlay_flag",
        "packed",
        OVERLAY_FLAG.trailing_zeros(),
        0x1,
    );
    lane(
        &mut text,
        "vtx_chroma_hi",
        "packed",
        CHROMA_HI_SHIFT,
        CHROMA_HI_MASK,
    );
    lane(&mut text, "vtx_block_red", "packed2", 0, BLOCK_LIGHT_MASK);
    lane(
        &mut text,
        "vtx_cell_u",
        "packed2",
        CELL_UV_U_SHIFT,
        CELL_UV_MASK,
    );
    lane(
        &mut text,
        "vtx_cell_v",
        "packed2",
        CELL_UV_V_SHIFT,
        CELL_UV_MASK,
    );
    lane(
        &mut text,
        "vtx_fluid_medium",
        "packed2",
        FLUID_MEDIUM_SHIFT,
        FLUID_MEDIUM_MASK,
    );
    lane(
        &mut text,
        "vtx_fluid_flow",
        "packed2",
        FLUID_FLOW_FLAG2.trailing_zeros(),
        0x1,
    );
    lane(
        &mut text,
        "vtx_normal_code",
        "packed2",
        NORMAL_CODE_SHIFT,
        NORMAL_CODE_MASK,
    );
    lane(
        &mut text,
        "vtx_dyed",
        "packed2",
        DYED_FLAG2.trailing_zeros(),
        0x1,
    );
    lane(
        &mut text,
        "vtx_overlay_payload",
        "packed2",
        OVERLAY_SHIFT2,
        OVERLAY_MASK,
    );
    lane(
        &mut text,
        "vtx_greedy_payload",
        "packed2",
        OVERLAY_SHIFT2,
        (SPAN_NIBBLE << 4) | SPAN_NIBBLE,
    );
    let lo = UV_TURN_LO_FLAG.trailing_zeros();
    let hi = UV_TURN_HI_FLAG2.trailing_zeros();
    writeln!(
        text,
        "fn vtx_uv_turn(packed: u32, packed2: u32) -> u32 {{\n    \
         return ((packed >> {lo}u) & 0x1u) | (((packed2 >> {hi}u) & 0x1u) << 1u);\n}}"
    )
    .unwrap();
    text.push_str(
        "fn vtx_cell_uv(packed2: u32) -> vec2<f32> {\n    \
         return vec2<f32>(f32(vtx_cell_u(packed2)), f32(vtx_cell_v(packed2))) / 16.0;\n}\n",
    );
    let span_hi = OVERLAY_SHIFT2 + 4;
    writeln!(
        text,
        "fn vtx_greedy_span(packed2: u32) -> vec2<f32> {{\n    \
         return vec2<f32>(f32(((packed2 >> {OVERLAY_SHIFT2}u) & {SPAN_NIBBLE:#X}u) + 1u), \
         f32(((packed2 >> {span_hi}u) & {SPAN_NIBBLE:#X}u) + 1u));\n}}"
    )
    .unwrap();
    // Light RGB is split across words, see `BlockLightVertexExt`.
    // Red comes from `packed2`.
    // Green and blue come from the chroma word: its low byte is the tint alpha lane, its high
    // nibble is in `packed`.
    // Each channel is stored XOR'd with red.
    writeln!(
        text,
        "fn block_light_rgb(packed: u32, packed2: u32, chroma_lo: f32) -> vec3<f32> {{\n    \
         let r = vtx_block_red(packed2);\n    \
         let chroma = u32(round(chroma_lo * 255.0)) | (vtx_chroma_hi(packed) << {CHROMA_LO_BITS}u);\n    \
         let g = (chroma & {LIGHT_BITS:#X}u) ^ r;\n    \
         let b = ((chroma >> 6u) & {LIGHT_BITS:#X}u) ^ r;\n    \
         return vec3<f32>(f32(r), f32(g), f32(b)) / 63.0;\n}}"
    )
    .unwrap();
    text
}

#[cfg(test)]
mod tests;
