use mod_sdk::*;

use machine_core::StepCtx;

use crate::anvil::{AnvilSpec, CellState, ACC_AUGMENT, ACC_NONE, ACC_SOCKET, SLOT_TOOL, SOCKETS};
use crate::augments::{condition_word, level_word, repairable, Entry, LEVEL_MAX};
use crate::keys::anvil as keys;

impl AnvilSpec {
    pub(super) fn publish_stage(&self, ctx: &StepCtx<'_>, slots: &[Option<ItemStackData>]) {
        let tool_stack = slots[SLOT_TOOL].as_ref().filter(|s| s.count > 0);
        let tool_name = tool_stack.map(|s| s.item.as_str()).unwrap_or("");
        let tool = self.tool_in(slots);
        let staged = self.staged(slots);

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

        let hint = if tool_stack.is_none() {
            "Add a tool".to_owned()
        } else {
            match staged.iter().find(|(_, fit, have)| have < &fit.cost) {
                Some((_, fit, _)) => format!("Needs {}", fit.cost),
                None => String::new(),
            }
        };

        let (speed_mult, damage_mult, knockback_mult) =
            staged
                .iter()
                .fold((1.0f32, 1.0f32, 1.0f32), |(s, d, k), (_, fit, _)| {
                    (
                        s * fit.speed_mult,
                        d * fit.damage_mult,
                        k * fit.knockback_mult,
                    )
                });
        let gentle = staged
            .iter()
            .filter_map(|(_, fit, _)| fit.gentle)
            .max()
            .map(|chance| format!("Gentle Mine +{chance}%"))
            .unwrap_or_default();

        ctx.publish(keys::TOOL_VIEW, GuiValue::Str(layers.join(",")));
        ctx.publish(keys::ANVIL_HINT, GuiValue::Str(hint));
        ctx.publish(
            keys::PREVIEW_SPEED,
            GuiValue::Str(delta_line("Speed", speed_mult)),
        );
        ctx.publish(
            keys::PREVIEW_DAMAGE,
            GuiValue::Str(delta_line("Damage", damage_mult)),
        );
        ctx.publish(
            keys::PREVIEW_KNOCKBACK,
            GuiValue::Str(delta_line("Knockback", knockback_mult)),
        );
        ctx.publish(keys::PREVIEW_GENTLE, GuiValue::Str(gentle));
        ctx.publish(
            keys::CAN_AUGMENT,
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
                CellState::Occupied(id) => {
                    let acc = entry.map_or(ACC_NONE, |e| {
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
            let cell = &keys::SOCKET_CELLS[socket];
            ctx.publish(cell.st, GuiValue::I32(frame));
            ctx.publish(cell.ghost, GuiValue::Str(ghost));
            ctx.publish(cell.acc, GuiValue::I32(acc));
            let tip = match state {
                CellState::Occupied(_) => entry.map(|e| self.socket_tip(e)).unwrap_or_default(),
                _ => String::new(),
            };
            ctx.publish(&format!("slot{}:tip", socket + 1), GuiValue::Str(tip));
        }
    }

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

fn span(palette: &str, text: &str) -> String {
    format!("{palette}|{}", text.replace(['|', '\t', '\n'], ""))
}

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

fn delta_line(label: &str, mult: f32) -> String {
    let delta = ((mult - 1.0) * 100.0).round() as i32;
    if delta == 0 {
        String::new()
    } else {
        format!("{label} {delta:+}%")
    }
}
