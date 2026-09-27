//! Keep the shader's field constants generated from the CPU field's source.
//! The pack installer copies this shader after the mod build finishes.

#[path = "../weather-core/src/field_constants.rs"]
#[allow(dead_code)]
mod field_constants;

use std::fs;
use std::path::Path;

const BEGIN: &str = "// BEGIN GENERATED WEATHER FIELD CONSTANTS";
const END: &str = "// END GENERATED WEATHER FIELD CONSTANTS";

fn main() {
    let shader = Path::new(env!("CARGO_MANIFEST_DIR")).join("pack/shaders/clouds.wgsl");
    println!("cargo:rerun-if-changed=../weather-core/src/field_constants.rs");
    println!("cargo:rerun-if-changed={}", shader.display());

    let source = fs::read_to_string(&shader).expect("read clouds.wgsl");
    let (prefix, rest) = source
        .split_once(BEGIN)
        .expect("weather constants begin marker");
    let (newline, rest) = if let Some(rest) = rest.strip_prefix("\r\n") {
        ("\r\n", rest)
    } else {
        (
            "\n",
            rest.strip_prefix('\n')
                .expect("weather constants begin marker newline"),
        )
    };
    let (_, suffix) = rest.split_once(END).expect("weather constants end marker");
    let constants = [
        format!("const WRAP: f32 = {:?};", field_constants::WRAP),
        format!(
            "const SHEET_B_FEATURE: f32 = {:?};",
            field_constants::SHEET_B_FEATURE
        ),
        format!(
            "const SHEET_B_ADVECT: f32 = {:?};",
            field_constants::SHEET_B_ADVECT
        ),
        format!(
            "const SHEET_B_SALT: u32 = 0x{:08X}u;",
            field_constants::SHEET_B_SALT
        ),
        format!("const RAIN_RAMP: f32 = {:?};", field_constants::RAIN_RAMP),
    ]
    .join(newline);
    let generated = format!("{prefix}{BEGIN}{newline}{constants}{newline}{END}{suffix}");
    if generated != source {
        fs::write(&shader, generated).expect("update generated shader constants");
    }
}
