use super::super::*;
use super::{lane_decoders, layout};

fn shader_bit_reads(src: &str) -> Vec<(&'static str, u32, u32)> {
    let hex = |s: &str| -> Option<u32> {
        let end = s.find('u')?;
        u32::from_str_radix(&s[..end], 16).ok()
    };
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = src[at..].find("packed") {
        let start = at + i;
        at = start + "packed".len();
        let word = if src[at..].starts_with('2') {
            at += 1;
            "packed2"
        } else {
            "packed"
        };
        let tail = src[at..].trim_start();
        if let Some(rest) = tail.strip_prefix(">> ") {
            let Some(u) = rest.find("u) & 0x") else {
                continue;
            };
            let (Ok(shift), Some(mask)) = (rest[..u].parse::<u32>(), hex(&rest[u + 7..])) else {
                continue;
            };
            out.push((word, shift, mask));
        } else if let Some(rest) = tail.strip_prefix("& 0x") {
            if let Some(mask) = hex(rest) {
                out.push((word, 0, mask));
            }
        }
    }
    out
}

#[test]
fn the_generated_module_decodes_exactly_the_rust_lanes() {
    let lanes: &[(&str, u32, u32, &str)] = &[
        ("packed", 0, TILE_MASK, "tile id"),
        ("packed", CORNER_SHIFT, 0x3, "corner"),
        ("packed", SHADE_SHIFT, 0x3, "shade index"),
        ("packed", AO_SHIFT, 0x3, "ao"),
        ("packed", SKY_SHIFT, 0x3F, "skylight"),
        ("packed", UV_MODE_SHIFT, 0x7, "uv mode"),
        ("packed", OVERLAY_FLAG.trailing_zeros(), 0x1, "has-overlay"),
        (
            "packed",
            CHROMA_HI_SHIFT,
            CHROMA_HI_MASK,
            "chroma high nibble",
        ),
        ("packed", 31, 0x1, "uv turn low bit"),
        ("packed2", 0, BLOCK_LIGHT_MASK, "block light red"),
        ("packed2", CELL_UV_U_SHIFT, CELL_UV_MASK, "cell-local u"),
        ("packed2", CELL_UV_V_SHIFT, CELL_UV_MASK, "cell-local v"),
        (
            "packed2",
            FLUID_MEDIUM_SHIFT,
            FLUID_MEDIUM_MASK,
            "fluid medium",
        ),
        (
            "packed2",
            FLUID_FLOW_FLAG2.trailing_zeros(),
            0x1,
            "fluid flow strip",
        ),
        (
            "packed2",
            NORMAL_CODE_SHIFT,
            NORMAL_CODE_MASK,
            "normal code",
        ),
        ("packed2", DYED_FLAG2.trailing_zeros(), 0x1, "dyed flag"),
        ("packed2", 31, 0x1, "uv turn high bit"),
        ("packed2", OVERLAY_SHIFT2, OVERLAY_MASK, "overlay tile"),
        ("packed2", OVERLAY_SHIFT2, 0xF, "greedy width"),
        ("packed2", OVERLAY_SHIFT2 + 4, 0xF, "greedy height"),
        (
            "packed2",
            OVERLAY_SHIFT2,
            0xFF,
            "greedy span (both nibbles)",
        ),
    ];
    let reads = shader_bit_reads(&lane_decoders());
    for &(word, shift, mask) in &reads {
        assert!(
            lanes
                .iter()
                .any(|&(w, s, m, _)| w == word && s == shift && m == mask),
            "the generated module decodes `{word}` at shift {shift} mask {mask:#X}, \
             which is not a lane mesh::vertex defines"
        );
    }
    for &(word, shift, mask, what) in lanes {
        assert!(
            reads.contains(&(word, shift, mask)),
            "the generated module no longer decodes the {what} lane ({word} >> {shift} & {mask:#X})"
        );
    }
}

#[test]
fn the_generated_transition_decode_reads_the_lanes_apply_writes() {
    let text = super::super::transition::wgsl();
    let reads = shader_bit_reads(&text);
    for want in [
        ("packed", 0, TILE_MASK),
        ("packed", OVERLAY_FLAG.trailing_zeros(), 0x1),
        ("packed2", CELL_UV_U_SHIFT, 0x3FF),
        ("packed2", DYED_FLAG2.trailing_zeros(), 0x1FF),
        ("packed", UV_MODE_SHIFT, 0x7),
        ("packed", SHADE_SHIFT, 0x3),
    ] {
        assert!(reads.contains(&want), "transition decode misses {want:?}");
    }
    assert!(
        text.contains("packed2 >> 28u"),
        "high nibble starts at bit 28"
    );
    assert!(
        text.contains("(packed >> 31u) << 12u"),
        "packed bit 31 is slot bit 12"
    );
}

#[test]
fn the_generated_module_declares_every_helper() {
    let text = layout();
    for helper in [
        "fn vtx_tile(",
        "fn vtx_corner(",
        "fn vtx_shade(",
        "fn vtx_ao(",
        "fn vtx_sky(",
        "fn vtx_uv_mode(",
        "fn vtx_overlay_flag(",
        "fn vtx_uv_turn(",
        "fn vtx_cell_uv(",
        "fn vtx_fluid_medium(",
        "fn vtx_fluid_flow(",
        "fn vtx_normal_code(",
        "fn vtx_dyed(",
        "fn vtx_overlay_payload(",
        "fn vtx_greedy_span(",
        "fn vtx_greedy_payload(",
        "fn block_light_rgb(",
        "fn transition_words(",
        "fn face_shade_idx(",
        "const UV_MODE_CELL_LOCAL: u32",
        "const NORMAL_POS_Y: u32",
    ] {
        assert!(text.contains(helper), "petramond::vertex lacks `{helper}`");
    }
}
