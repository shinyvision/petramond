use super::*;

fn validate(label: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{label} fails to parse: {}", e.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{label} fails validation: {e:?}"));
}

/// Every engine shader source, by file name.
fn engine_shaders() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
    let mut out: Vec<_> = std::fs::read_dir(&dir)
        .expect("shader directory")
        .map(|entry| entry.expect("shader entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "wgsl"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&path).expect("shader reads"),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_source_without_imports_is_borrowed_untouched() {
    let src = "fn f() -> f32 { return 1.0; }\n";
    assert!(matches!(compose(src), Ok(Cow::Borrowed(s)) if s == src));
}

#[test]
fn an_unknown_import_names_its_line_and_module() {
    let err = compose("// header\n#import petramond::nope\n").unwrap_err();
    assert_eq!(
        err,
        ImportError {
            line: 2,
            module: "petramond::nope".to_owned()
        }
    );
    assert!(err.to_string().contains("petramond::frame"), "{err}");
}

/// Concatenated pieces commonly import the same module; it must land once
/// (WGSL rejects a redeclared struct).
#[test]
fn a_module_imported_twice_is_emitted_once() {
    let text = compose("#import petramond::frame\n#import petramond::frame\n").unwrap();
    assert_eq!(text.matches("struct Uniforms").count(), 1);
    validate("frame twice", &text);
}

/// Each generated module is valid WGSL on its own, and all of them together.
#[test]
fn every_module_composes_to_valid_wgsl() {
    let mut all = String::new();
    for module in MODULES {
        let import = format!("#import {module}\n");
        validate(module, &compose(&import).unwrap());
        all.push_str(&import);
    }
    validate("every module", &compose(&all).unwrap());
}

/// The point of the module system: no engine shader spells a generated
/// declaration by hand. A struct copy is how a shader silently read the
/// wrong frame field; a raw `packed >> N` is how a lane drifted from the
/// mesher's layout.
#[test]
fn no_engine_shader_hand_copies_a_generated_declaration() {
    for (name, src) in engine_shaders() {
        for forbidden in [
            "struct Uniforms",
            "struct FrameUniforms",
            "struct ShaderParams",
            "struct TransitionWords",
            "uv_rects: array",
            "fn block_light_rgb",
            "const UV_MODE_",
            "const NORMAL_",
        ] {
            assert!(
                !src.contains(forbidden),
                "{name} declares `{forbidden}` by hand — import the generated module"
            );
        }
        for word in ["packed", "packed2"] {
            for op in [" >> ", " & 0x"] {
                let needle = format!("{word}{op}");
                assert!(
                    !src.contains(&needle),
                    "{name} decodes `{needle}…` by hand — use the petramond::vertex helpers"
                );
            }
        }
    }
}

/// The engine shaders whose pipelines compose a fixed source parse and
/// validate as composed (the terrain, particle and skinned shaders are
/// covered with their generated tables by their own modules' tests; the GPU
/// factory test covers the rest).
#[test]
fn composed_engine_shaders_validate() {
    let cel = include_str!("../../../shaders/cel.wgsl");
    let atmosphere = include_str!("../../../shaders/atmosphere.wgsl");
    for (label, source) in [
        (
            "model3d",
            include_str!("../../../shaders/model3d.wgsl").to_owned(),
        ),
        (
            "break overlay",
            [
                cel,
                atmosphere,
                include_str!("../../../shaders/break_overlay.wgsl"),
            ]
            .concat(),
        ),
        (
            "contact",
            [
                cel,
                atmosphere,
                include_str!("../../../shaders/contact.wgsl"),
            ]
            .concat(),
        ),
        (
            "entity shadow",
            [
                cel,
                atmosphere,
                include_str!("../../../shaders/entity_shadow.wgsl"),
            ]
            .concat(),
        ),
        (
            "outline",
            include_str!("../../../shaders/outline.wgsl").to_owned(),
        ),
        (
            "sky",
            [cel, atmosphere, include_str!("../../../shaders/sky.wgsl")].concat(),
        ),
    ] {
        validate(label, &compose(&source).unwrap());
    }
}
