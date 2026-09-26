use super::*;

/// Byte size of a frame-uniform field's WGSL type (uniform address space).
fn wgsl_size(ty: &str) -> usize {
    match ty {
        "mat4x4<f32>" => 64,
        "vec4<f32>" | "vec4<u32>" | "vec4<i32>" => 16,
        other => panic!("unhandled frame uniform type `{other}`"),
    }
}

/// The generated `petramond::frame` struct is laid out from its own
/// declaration, so the table it is generated from must describe [`Uniforms`]
/// exactly: every field at its Rust offset, back to back, and together the
/// whole struct. A field added to `Uniforms` but not to the table leaves a
/// gap (or a short total) here instead of a shader silently reading the
/// wrong bytes.
#[test]
fn the_frame_table_describes_every_uniform_byte() {
    let mut offset = 0usize;
    for (name, ty, at) in UNIFORM_FIELDS {
        assert_eq!(
            at, offset,
            "`{name}` sits at byte {at} in `Uniforms` but the table reaches it at {offset} \
             (a field missing from UNIFORM_FIELDS?)"
        );
        offset += wgsl_size(ty);
    }
    assert_eq!(
        offset,
        std::mem::size_of::<Uniforms>(),
        "UNIFORM_FIELDS stops short of the end of `Uniforms`"
    );
}

/// The generated module declares every table field, in order.
#[test]
fn the_frame_module_declares_the_table_in_order() {
    let text = frame_wgsl();
    let mut at = 0;
    for (name, ty, _) in UNIFORM_FIELDS {
        let decl = format!("{name}: {ty},");
        let found = text[at..]
            .find(&decl)
            .unwrap_or_else(|| panic!("petramond::frame lacks `{decl}` (or out of order)"));
        at += found + decl.len();
    }
    assert!(text.contains(&format!("PETRAMOND_FRAME_ABI: u32 = {FRAME_ABI_VERSION}u")));
}

/// The shader-param slot count reaches WGSL through the generated module.
#[test]
fn the_shader_params_module_spells_the_slot_count() {
    assert!(shader_params_wgsl().contains(&format!("array<vec4<f32>, {SHADER_PARAM_SLOTS}>")));
    assert_eq!(std::mem::size_of::<ShaderParams>(), SHADER_PARAM_SLOTS * 16);
}
