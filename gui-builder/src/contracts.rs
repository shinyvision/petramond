//! The engine's per-kind slot contracts, replicated from the game's
//! `src/gui/documents.rs` so builder validation matches what the game will
//! accept at load time. Keep the two tables in sync by hand — the builder is
//! deliberately outside the game workspace.

use llama_ui::{DocClass, SlotContract};

/// Every engine document kind the builder knows how to scaffold.
pub const ENGINE_KINDS: &[&str] = &[
    "llama:chest",
    "llama:inventory",
    "llama:crafting_table",
    "llama:furnace",
    "llama:hotbar",
    "llama:furniture_workbench",
    "llama:title",
    "llama:world_select",
    "llama:world_settings",
    "llama:create_world",
    "llama:delete_world",
    "llama:pause",
    "llama:demo",
];

/// The engine's slot expectations for a document kind. Unknown (mod) kinds and
/// shell screens carry no role slots.
pub fn contract_for(kind: &str) -> SlotContract {
    match kind {
        "llama:chest" => SlotContract::new(&[("storage", 27), ("player_inv", 27), ("hotbar", 9)]),
        "llama:inventory" => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("craft_input", 4),
            ("craft_result", 1),
        ]),
        "llama:crafting_table" => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("craft_input", 9),
            ("craft_result", 1),
        ]),
        "llama:furnace" => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("furnace_input", 1),
            ("furnace_fuel", 1),
            ("furnace_output", 1),
        ]),
        "llama:hotbar" => SlotContract::new(&[("hotbar", 9)]),
        "llama:furniture_workbench" => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("workbench_input", 1),
            ("workbench_result", 21),
        ]),
        "llama:demo" => SlotContract::new(&[("demo_slots", 9)]),
        _ => SlotContract::default(),
    }
}

/// The document class the engine expects for a kind (mod kinds default to
/// `Screen`; authors can change it).
pub fn class_for(kind: &str) -> DocClass {
    match kind {
        "llama:chest"
        | "llama:inventory"
        | "llama:crafting_table"
        | "llama:furnace"
        | "llama:furniture_workbench" => DocClass::Container,
        "llama:hotbar" => DocClass::Hud,
        _ => DocClass::Screen,
    }
}

/// A sensible grid shape for `count` slots of `role` (used when scaffolding a
/// new document that must satisfy its contract).
pub fn default_grid(count: usize) -> (u32, u32) {
    match count {
        27 => (9, 3),
        9 => (3, 3),
        4 => (2, 2),
        21 => (7, 3),
        1 => (1, 1),
        n => (n as u32, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_table_matches_engine_expectation() {
        // Hand-written expectation mirroring src/gui/documents.rs — if this
        // fails, the builder and the game disagree about what validates.
        let expect: &[(&str, &[(&str, usize)])] = &[
            ("llama:chest", &[("storage", 27), ("player_inv", 27), ("hotbar", 9)]),
            (
                "llama:inventory",
                &[("player_inv", 27), ("hotbar", 9), ("craft_input", 4), ("craft_result", 1)],
            ),
            (
                "llama:crafting_table",
                &[("player_inv", 27), ("hotbar", 9), ("craft_input", 9), ("craft_result", 1)],
            ),
            (
                "llama:furnace",
                &[
                    ("player_inv", 27),
                    ("hotbar", 9),
                    ("furnace_input", 1),
                    ("furnace_fuel", 1),
                    ("furnace_output", 1),
                ],
            ),
            ("llama:hotbar", &[("hotbar", 9)]),
            (
                "llama:furniture_workbench",
                &[("player_inv", 27), ("hotbar", 9), ("workbench_input", 1), ("workbench_result", 21)],
            ),
            ("llama:demo", &[("demo_slots", 9)]),
        ];
        for (kind, roles) in expect {
            assert_eq!(contract_for(kind), SlotContract::new(roles), "{kind}");
        }
        // Shell screens and mod kinds carry no slots.
        for kind in ["llama:pause", "llama:title", "llama:world_select", "somemod:wheel"] {
            assert!(contract_for(kind).roles.is_empty(), "{kind}");
        }
    }
}
