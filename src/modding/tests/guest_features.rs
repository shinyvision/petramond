//! The wasm feature contract between the guest build and the host engine:
//! bundled mods are compiled with core SIMD128 (`mods-src/.cargo/config.toml`)
//! and the engine must accept it, while relaxed SIMD stays off for
//! determinism. Both sides are asserted here so neither drifts on its own.

use wasmtime::Module;

/// The guest workspace's cargo config, which sets the wasm target features
/// every bundled mod is compiled with.
const GUEST_CARGO_CONFIG: &str = include_str!("../../../mods-src/.cargo/config.toml");

#[test]
fn bundled_guests_are_built_with_core_simd128() {
    let section = GUEST_CARGO_CONFIG
        .split("[target.wasm32-unknown-unknown]")
        .nth(1)
        .expect("mods-src/.cargo/config.toml configures the wasm32 target");
    let rustflags = section
        .lines()
        .find(|line| line.trim_start().starts_with("rustflags"))
        .expect("the wasm32 target section sets rustflags");
    assert!(
        rustflags.contains("+simd128"),
        "guests must enable core SIMD128 (the engine runs it): {rustflags}"
    );
    assert!(
        !rustflags.contains("relaxed-simd"),
        "relaxed SIMD is nondeterministic and disabled in the engine: {rustflags}"
    );
}

#[test]
fn the_engine_accepts_core_simd_guests() {
    let wat = r#"(module
  (func (export "lanes") (result i32)
    (i32x4.extract_lane 3
      (i32x4.add (v128.const i32x4 1 2 3 4) (v128.const i32x4 10 20 30 40)))))"#;
    Module::new(super::super::host::engine(), wat.as_bytes())
        .expect("a guest using core SIMD128 compiles on the host engine");
}

#[test]
fn the_engine_refuses_relaxed_simd_guests() {
    let wat = r#"(module
  (func (export "swizzle") (result v128)
    (i8x16.relaxed_swizzle (v128.const i64x2 0 0) (v128.const i64x2 0 0))))"#;
    assert!(
        Module::new(super::super::host::engine(), wat.as_bytes()).is_err(),
        "relaxed SIMD must stay disabled on the host engine"
    );
}
