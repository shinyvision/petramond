//! The WRITER side of the engine's item instance-data vocabulary.
//!
//! A mod that transforms a stack (the forge anvil) writes engine-read keys
//! into the stack's data map. The engine parses these leniently, so any
//! hand-formatted string "works" — but then the wire format lives as a
//! convention scattered across format strings. These helpers make each key's
//! value ONE function, so every writer produces byte-identical output for
//! equal inputs. Byte identity is load-bearing: a stack's variant identity IS
//! its canonical data bytes, so two equally-augmented tools stack together
//! only if their writers agreed to the byte.

/// The engine's per-stack tool override key: a JSON override of the row's
/// resolved tool properties. Write values with [`tool_override_json`]; the
/// reader-side contract lives on the engine's `TOOL_DATA_KEY`.
pub const TOOL_OVERRIDE_KEY: &str = "petramond:tool";

/// The engine's sprite-overlay key: a comma-separated ORDERED list of item
/// registry names whose sprites composite over the stack's own, first name
/// drawn first (bottom-most), at every render site. Overlay art is authored
/// IN POSITION on its own transparent tile, so the list is the entire
/// compositing instruction — no coordinates anywhere.
pub const OVERLAY_DATA_KEY: &str = "petramond:overlay";

/// The [`TOOL_OVERRIDE_KEY`] value for stamped-absolute tool stats.
///
/// Stats are quantized to 4 decimal places — a deliberate precision decision,
/// not a formatting accident: ample for any stat the ladder produces, and it
/// keeps equal stat sets stamping byte-identical JSON regardless of how a
/// writer computed them, which is what lets equally-augmented stacks merge.
pub fn tool_override_json(tier: u8, speed: f32, damage: [f32; 2]) -> String {
    format!(
        "{{\"tier\":{tier},\"speed\":{speed:.4},\"damage\":[{:.4},{:.4}]}}",
        damage[0], damage[1]
    )
}
