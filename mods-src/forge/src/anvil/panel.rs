//! What the open panel SHOWS: the enlarged tool stage, the result preview,
//! the Augment button's enabled state, and per socket cell its chrome, ghost
//! icon, admission mask and tooltip.
//!
//! Publishing only — every value here is derived from the slots the step just
//! settled, so the panel can never show a stale tool.

use mod_sdk::*;

use machine_core::StepCtx;

use crate::anvil::{AnvilSpec, CellState, ACC_AUGMENT, ACC_NONE, ACC_SOCKET, SLOT_TOOL, SOCKETS};
use crate::augments::{condition_word, level_word, repairable, Entry, LEVEL_MAX};

/// A socket cell's gui-state key. The panel document binds these by hand, so
/// the test that pins the document against this module builds its expectations
/// from here rather than restating the format.
pub(super) fn sock_key(socket: usize, part: &str) -> String {
    format!("forge:sock{socket}_{part}")
}

impl AnvilSpec {
    /// Publish the panel: the enlarged tool (a composited LAYER LIST on one
    /// item-view hook), the stage hint, the result PREVIEW (stat delta lines
    /// aggregated over every staged fit), the Augment button's enabled
    /// state, and each socket cell's chrome (lock / cover frame index) and
    /// ghost (the grayed material icon of an installed augment, `~`-dimmed).
    /// Published every open tick so the panel can never show a stale tool —
    /// values persist in the state map until overwritten, and an empty
    /// string both blanks and HIDES its label (`visible` binds the same
    /// key).
    pub(super) fn publish_stage(&self, ctx: &StepCtx<'_>, slots: &[Option<ItemStackData>]) {
        let tool_stack = slots[SLOT_TOOL].as_ref().filter(|s| s.count > 0);
        let tool_name = tool_stack.map(|s| s.item.as_str()).unwrap_or("");
        let tool = self.tool_in(slots);
        let staged = self.staged(slots);

        // The stage previews the RESULT: the tool's sprite, then exactly
        // what the engine composites in the world (the stack's own stamped
        // art list), then the overlays the staged materials WOULD apply — so
        // inserting a diamond shows the augmented tool before the player
        // commits. A staged fit never collides with an installed one:
        // `staged` only fits open, free cells.
        let mut layers = vec![tool_name.to_owned()];
        let applied = tool_stack
            .and_then(|s| s.data.iter().find(|(k, _)| k == OVERLAY_DATA_KEY))
            .and_then(|(_, v)| std::str::from_utf8(v).ok())
            .unwrap_or("");
        if !applied.is_empty() {
            layers.push(applied.to_owned());
        }
        if let Some((_, tool_slots, _, _)) = &tool {
            for (_, fit, _) in &staged {
                layers.push(fit.overlay_for(&tool_slots.family).to_owned());
            }
        }

        // The one state the player cannot see from the slots themselves: a
        // valid material that is short of its cost.
        let hint = if tool_stack.is_none() {
            "Insert a tool".to_owned()
        } else {
            match staged.iter().find(|(_, fit, have)| have < &fit.cost) {
                Some((_, fit, _)) => format!("Needs {}", fit.cost),
                None => String::new(),
            }
        };

        // The preview shows for every staged fit, affordable or not — the
        // hint covers the shortfall; only the button gates on the cost.
        let (speed_mult, damage_mult) =
            staged.iter().fold((1.0f32, 1.0f32), |(s, d), (_, fit, _)| {
                (s * fit.speed_mult, d * fit.damage_mult)
            });
        // A behaviour grant is a preview line too — a stat panel that shows
        // only the -20% half of the gold inlay reads as a downgrade.
        let gentle = staged
            .iter()
            .filter_map(|(_, fit, _)| fit.gentle)
            .max()
            .map(|chance| format!("Gentle Mine +{chance}%"))
            .unwrap_or_default();

        ctx.publish("forge:tool_view", GuiValue::Str(layers.join(",")));
        ctx.publish("forge:anvil_hint", GuiValue::Str(hint));
        ctx.publish(
            "forge:preview_speed",
            GuiValue::Str(delta_line("Speed", speed_mult)),
        );
        ctx.publish(
            "forge:preview_damage",
            GuiValue::Str(delta_line("Damage", damage_mult)),
        );
        ctx.publish("forge:preview_gentle", GuiValue::Str(gentle));
        ctx.publish(
            "forge:can_augment",
            GuiValue::I32(self.apply_staged(slots).is_some() as i32),
        );

        // Per-cell chrome + ghost + ADMISSION mask. The chrome frame indexes
        // the panel's 3-cell state sheet (0 nothing, 1 lock, 2 no-socket
        // cover) and is published only while the cell is EMPTY — never paint
        // over a real stack. The ghost is the installed augment's MATERIAL,
        // `~`-dimmed (the layer-list ghost marker), shown while its cell is
        // empty. The mask (`bind.accepts`) is what the cell currently TAKES,
        // enforced by the engine on clicks, drags, shift-routing and their
        // predictions: an open socket takes augment materials, a locked one
        // takes only the carving gem, occupied and absent cells take nothing
        // — as if the slot did not exist.
        for socket in 0..SOCKETS {
            let cell_empty = slots[socket + 1].as_ref().filter(|s| s.count > 0).is_none();
            let state = match &tool {
                Some((_, tool_slots, _, rec)) => Self::cell_state(tool_slots, rec, socket),
                None => CellState::Absent,
            };
            let entry = tool
                .as_ref()
                .and_then(|(_, _, _, rec)| rec.entry_at(socket));
            let (frame, ghost, acc) = match state {
                // An occupied socket takes its own material (repair, while
                // short of full) and the socket gem (mount upgrade, while
                // short of Legendary); the step adjudicates exact identity
                // and sweeps a mismatch home.
                CellState::Occupied(id) => {
                    let acc = entry.map_or(ACC_NONE, |e| {
                        // A pristine augment refuses its material outright —
                        // the mask says so on both mirrors, so the repair
                        // gesture only ever lands where it will act.
                        let repair = if repairable(e.cond, e.lvl) {
                            ACC_AUGMENT
                        } else {
                            ACC_NONE
                        };
                        let upgrade = if e.lvl < LEVEL_MAX {
                            ACC_SOCKET
                        } else {
                            ACC_NONE
                        };
                        repair | upgrade
                    });
                    (
                        0,
                        self.material_of
                            .get(id)
                            .map(|m| format!("~{m}"))
                            .unwrap_or_default(),
                        acc,
                    )
                }
                CellState::Open => (0, String::new(), ACC_AUGMENT),
                CellState::Locked => (1, String::new(), ACC_SOCKET),
                CellState::Absent => (2, String::new(), ACC_NONE),
            };
            let frame = if cell_empty { frame } else { 0 };
            let ghost = if cell_empty { ghost } else { String::new() };
            ctx.publish(&sock_key(socket, "st"), GuiValue::I32(frame));
            ctx.publish(&sock_key(socket, "ghost"), GuiValue::Str(ghost));
            ctx.publish(&sock_key(socket, "acc"), GuiValue::I32(acc));
            // The socket tooltip (the engine's injected slot tip, keyed by
            // the CONTAINER cell index): mount level + augment name on the
            // level's colour, the condition word on its own.
            let tip = match state {
                CellState::Occupied(_) => entry.map(|e| self.socket_tip(e)).unwrap_or_default(),
                _ => String::new(),
            };
            ctx.publish(&format!("slot{}:tip", socket + 1), GuiValue::Str(tip));
        }
    }

    /// The two tooltip lines for an occupied socket, in the injected slot
    /// tip's span format (newline-separated lines of tab-separated spans):
    /// ONLY the level and condition WORDS carry colour — `"Great"` green
    /// then `" Diamond Tip"` plain, `"Condition: "` plain then `"Worn"`
    /// yellow (Rachel, 2026-08-10).
    pub(super) fn socket_tip(&self, e: &Entry) -> String {
        let (lvl, lvl_col) = level_word(e.lvl);
        let (cond, cond_col) = condition_word(e.cond, e.lvl);
        let name = self.names.get(&e.id).map(String::as_str).unwrap_or(&e.id);
        format!(
            "{}\t{}\n{}\t{}",
            span(palette_entry(lvl_col), lvl),
            span("text", &format!(" {name}")),
            span("text", "Condition: "),
            span(palette_entry(cond_col), cond),
        )
    }
}

/// One `palette|text` span. The three separators are STRUCTURAL in the tip
/// format — a `|` opens the text, a tab the next span, a newline the next
/// line — and a display name is row data this pack does not own, so the text
/// half is stripped of all three rather than trusted.
fn span(palette: &str, text: &str) -> String {
    format!("{palette}|{}", text.replace(['|', '\t', '\n'], ""))
}

/// The theme palette entry a tooltip colour word paints with.
fn palette_entry(color: &str) -> &'static str {
    match color {
        "white" => "text",
        "green" => "accent",
        "yellow" => "warn",
        "red" => "danger",
        "purple" => "arcane",
        _ => "gold",
    }
}

/// `"Speed +50%"` from a multiplier — empty (which hides the label) at 1.0.
fn delta_line(label: &str, mult: f32) -> String {
    let delta = ((mult - 1.0) * 100.0).round() as i32;
    if delta == 0 {
        String::new()
    } else {
        format!("{label} {delta:+}%")
    }
}
