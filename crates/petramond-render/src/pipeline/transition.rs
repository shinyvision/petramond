//! Texture-transition tables, resolved at pipeline construction into WGSL
//! constants: no per-frame buffer, and the shader indexes by the set and
//! local material ids the mesher encoded.

use petramond_world::texture_transition::{Rules, MAX_MATERIALS_PER_SET};
use std::fmt::Write;

/// Rows per set in the face table: local ids 0..=15, three face classes each.
const ROWS_PER_SET: usize = (MAX_MATERIALS_PER_SET + 1) * 3;

pub(super) fn declarations() -> String {
    declarations_for(petramond_world::texture_transition::rules())
}

/// `TRANSITION_SET[set] = (mask base tile, width texels, tinted, face-row
/// offset)`; `TRANSITION_FACES[offset + local * 3 + face] = (base tile,
/// overlay tile, flags)`, flags bit 0 = base tinted, 1 = has overlay, 2 =
/// overlay tinted. Local id 0 (no donor) rows stay zero. WGSL arrays need a
/// length, so a world without sets still declares one empty entry.
pub(super) fn declarations_for(rules: &Rules) -> String {
    let sets = rules.sets.len();
    let mut text = format!(
        "const TRANSITION_SET_COUNT: u32 = {sets}u;\nconst TRANSITION_SET = array<vec4<u32>, {}>(\n",
        sets.max(1)
    );
    if sets == 0 {
        text.push_str("vec4<u32>(0u),\n");
    }
    for (i, set) in rules.sets.iter().enumerate() {
        writeln!(
            text,
            "vec4<u32>({}u, {}u, {}u, {}u),",
            set.mask.id(),
            set.width_texels,
            u32::from(set.tint.is_some()),
            i * ROWS_PER_SET
        )
        .unwrap();
    }
    text.push_str(");\n");
    writeln!(
        text,
        "const TRANSITION_FACES = array<vec4<u32>, {}>(",
        (sets * ROWS_PER_SET).max(1)
    )
    .unwrap();
    if sets == 0 {
        text.push_str("vec4<u32>(0u),\n");
    }
    for set in &rules.sets {
        let mut rows = 0;
        for _ in 0..3 {
            text.push_str("vec4<u32>(0u),\n");
            rows += 1;
        }
        for material in &set.materials {
            for face in material.faces {
                let flags = u32::from(face.base.world_tint().is_some())
                    | (u32::from(face.overlay.is_some()) << 1)
                    | (u32::from(face.overlay.is_some_and(|t| t.world_tint().is_some())) << 2);
                writeln!(
                    text,
                    "vec4<u32>({}u, {}u, {flags}u, 0u),",
                    face.base.id(),
                    face.overlay.map_or(0, |t| t.id())
                )
                .unwrap();
                rows += 1;
            }
        }
        for _ in rows..ROWS_PER_SET {
            text.push_str("vec4<u32>(0u),\n");
        }
    }
    text.push_str(");\n");
    text
}
