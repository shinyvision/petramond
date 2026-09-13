//! The block shader's fluid medium table, generated from the fluid rows: a
//! third fluid renders from its block row with no shader edit.
//!
//! A fluid face carries `medium index + 1` in its vertex; `fluid_face(index)`
//! answers that medium's surface response. It is a generated `switch`, like the
//! flipbook table, so it grows with the fluid count without a GPU array bound.

use petramond_world::fluid::FluidMedium;
use std::fmt::Write;

/// The WGSL `FluidFace` row type and its lookup for `media` (position = index).
pub(super) fn declarations(media: &[FluidMedium]) -> String {
    assert!(
        media.len() <= petramond_mesh::MAX_FLUID_MEDIA as usize,
        "{} fluid media exceed the vertex lane's {}",
        media.len(),
        petramond_mesh::MAX_FLUID_MEDIA
    );
    let mut text = String::from(
        "struct FluidFace {\n    surface_tint: vec3<f32>,\n    surface_alpha: f32,\n    \
         albedo_toward: f32,\n    albedo_amount: f32,\n    sheen: bool,\n};\n\
         fn fluid_face(medium: u32) -> FluidFace {\n switch medium {\n",
    );
    for (index, m) in media.iter().enumerate() {
        let [r, g, b] = m.surface_tint;
        let mix = m
            .albedo_mix
            .map_or((0.0, 0.0), |mix| (mix.toward, mix.amount));
        writeln!(
            text,
            "case {index}u: {{ return FluidFace(vec3<f32>({r:?}, {g:?}, {b:?}), {:?}, {:?}, {:?}, {}); }}",
            m.surface_alpha, mix.0, mix.1, m.sheen
        )
        .unwrap();
    }
    text.push_str(
        "default: { return FluidFace(vec3<f32>(1.0, 1.0, 1.0), 1.0, 0.0, 0.0, false); }\n }\n}\n",
    );
    text
}

/// The registered fluids' media, in medium-index order.
pub(super) fn registered() -> Vec<FluidMedium> {
    petramond_world::fluid::medium::media()
        .iter()
        .filter_map(|block| block.fluid_def().map(|def| def.medium))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::fluid::AlbedoMix;

    fn medium(tint: [f32; 3], alpha: f32, sheen: bool) -> FluidMedium {
        FluidMedium {
            fog_color: [0.1, 0.2, 0.3],
            fog_start: 0.5,
            fog_end: 8.0,
            volume_tint: [0.9, 0.9, 0.9],
            surface_tint: tint,
            surface_alpha: alpha,
            albedo_mix: sheen.then_some(AlbedoMix {
                toward: 0.25,
                amount: 0.5,
            }),
            sheen,
            self_lit: 0.0,
            eye_margin: None,
        }
    }

    /// A fluid added as row data alone — here a third, acid-like medium —
    /// reaches the block shader: the composed source validates and addresses
    /// every medium at its own index.
    #[test]
    fn a_third_fluid_renders_from_its_row_alone() {
        let media = [
            medium([0.4, 0.6, 0.8], 0.78, true),
            medium([0.8, 0.5, 0.4], 1.0, false),
            medium([0.3, 0.9, 0.2], 0.6, false),
        ];
        let table = declarations(&media);
        let cases: Vec<&str> = table.lines().filter(|l| l.starts_with("case ")).collect();
        assert_eq!(cases.len(), media.len(), "one case per medium");
        for (index, m) in media.iter().enumerate() {
            // The row's own case, as a table of that medium alone spells it.
            let alone = declarations(std::slice::from_ref(m));
            let own = alone
                .lines()
                .find(|l| l.starts_with("case 0u:"))
                .expect("a one-medium table has case 0")
                .replacen("case 0u:", &format!("case {index}u:"), 1);
            assert_eq!(
                cases[index], own,
                "medium {index} is addressed by its own row"
            );
        }
        let source = super::super::block_shader_source(&media);
        let module = naga::front::wgsl::parse_str(&source).expect("block shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("block shader validates with three fluid media");
    }
}
