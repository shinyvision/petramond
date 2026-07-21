//! The engine's per-kind slot contracts, replicated from the game's
//! `src/gui/documents.rs` so builder validation matches what the game will
//! accept at load time. Keep the two tables in sync by hand — the builder is
//! deliberately outside the game workspace.

use petramond_ui::{DocClass, SlotContract};

/// Every engine document kind the builder knows how to scaffold.
pub const ENGINE_KINDS: &[&str] = &[
    "petramond:chest",
    "petramond:inventory",
    "petramond:crafting_table",
    "petramond:furnace",
    "petramond:hotbar",
    "petramond:furniture_workbench",
    "petramond:title",
    "petramond:world_select",
    "petramond:world_settings",
    "petramond:create_world",
    "petramond:delete_world",
    "petramond:pause",
    "petramond:demo",
];

/// The engine's slot expectations for a document kind. Unknown (mod) kinds and
/// shell screens carry no role slots.
pub fn contract_for(kind: &str) -> SlotContract {
    match kind {
        "petramond:chest" => SlotContract::new(&[("storage", 27), ("player_inv", 27), ("hotbar", 9)]),
        "petramond:inventory" => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("craft_result", 1),
        ]),
        "petramond:crafting_table" => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("craft_result", 1),
        ]),
        "petramond:furnace" => SlotContract::new(&[
            ("player_inv", 27),
            ("hotbar", 9),
            ("furnace_input", 1),
            ("furnace_fuel", 1),
            ("furnace_output", 1),
        ]),
        "petramond:hotbar" => SlotContract::new(&[("hotbar", 9)]),
        "petramond:demo" => SlotContract::new(&[("demo_slots", 9)]),
        _ => SlotContract::default(),
    }
}

/// The document class the engine expects for a kind (mod kinds default to
/// `Screen`; authors can change it).
pub fn class_for(kind: &str) -> DocClass {
    match kind {
        "petramond:chest"
        | "petramond:inventory"
        | "petramond:crafting_table"
        | "petramond:furnace"
        | "petramond:furniture_workbench" => DocClass::Container,
        "petramond:hotbar" => DocClass::Hud,
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
            ("petramond:chest", &[("storage", 27), ("player_inv", 27), ("hotbar", 9)]),
            (
                "petramond:inventory",
                &[("player_inv", 27), ("hotbar", 9), ("craft_result", 1)],
            ),
            (
                "petramond:crafting_table",
                &[("player_inv", 27), ("hotbar", 9), ("craft_result", 1)],
            ),
            (
                "petramond:furnace",
                &[
                    ("player_inv", 27),
                    ("hotbar", 9),
                    ("furnace_input", 1),
                    ("furnace_fuel", 1),
                    ("furnace_output", 1),
                ],
            ),
            ("petramond:hotbar", &[("hotbar", 9)]),
            ("petramond:demo", &[("demo_slots", 9)]),
        ];
        for (kind, roles) in expect {
            assert_eq!(contract_for(kind), SlotContract::new(roles), "{kind}");
        }
        // Shell screens and mod kinds carry no slots.
        for kind in ["petramond:pause", "petramond:title", "petramond:world_select", "somemod:wheel"] {
            assert!(contract_for(kind).roles.is_empty(), "{kind}");
        }
    }
}
