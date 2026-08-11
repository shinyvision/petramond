//! The gestures: every arithmetic that turns slots plus a record into a
//! restamped tool. Carving a socket, tending an occupied one (repair, mount
//! upgrade), staging what fits, applying it, wearing an augment down — and
//! [`AnvilSpec::stamp`], the one place the three keys on a tool are written.
//!
//! All of it is pure with respect to the machine: it reads slots and answers
//! new stacks, so `step` owns when anything happens (and owns the sounds —
//! these functions are driven directly by unit tests, where a host call is a
//! panic).

use std::collections::HashSet;

use mod_sdk::*;

use crate::anvil::rows::{Fit, ToolSlots, ToolStats, WearOn};
use crate::anvil::workstation::take;
use crate::anvil::{AnvilSpec, CellState, SLOTS, SLOT_TOOL, VALUE_CAP, WEAR_STREAM};
use crate::augments::{quanta_max, repairable, Entry, Record, AUGMENTS_KEY, LEVEL_MAX};

impl AnvilSpec {
    /// Carving: the restamped tool plus the cell to consume a socket
    /// material from, when a tool with locked sockets left shares the anvil
    /// with one.
    pub(super) fn carve(&self, slots: &[Option<ItemStackData>]) -> Option<(ItemStackData, usize)> {
        let (tool, tool_slots, stats, rec) = self.tool_in(slots)?;
        if rec.carved >= tool_slots.lockable {
            return None;
        }
        // A gem resting on an OCCUPIED socket is an upgrade gesture
        // ([`AnvilSpec::tend_sockets`]), never carve fuel — the cell it sits
        // in is what the gesture means.
        let cell = (1..SLOTS).find(|&c| {
            rec.id_at(c - 1).is_none()
                && slots[c]
                    .as_ref()
                    .is_some_and(|s| s.count > 0 && self.socket_items.contains(&s.item))
        })?;
        let carved = Record {
            carved: rec.carved + 1,
            entries: rec.entries.clone(),
        };
        let stamped = self.stamp(tool, tool_slots, stats, &carved)?;
        Some((stamped, cell))
    }

    /// Quanta one material restores when dropped on its augment's occupied
    /// socket: the INSTALL rate — `cost` materials buy the full base bar
    /// either way.
    fn repair_quanta(fit: &Fit) -> u16 {
        (100u16).div_ceil(fit.cost.max(1) as u16)
    }

    /// A qualifying wear event on `player`'s held tool: every installed,
    /// unbroken augment whose fit wears on this event class — plus the one
    /// whose own behaviour just fired, when `proc_id` names it — loses one
    /// condition quantum with probability `100 / max`, so the fit's `max`
    /// sets the expected pool in EVENTS while the stored space stays tiny.
    /// Any loss restamps the tool in place through the held-stack
    /// compare-and-set, against the very data map this read: a hand swapped
    /// since the event, or a stack another writer re-stamped in between,
    /// refuses harmlessly instead of clobbering. Streams advance only on
    /// qualifying augments (the gold.rs rule).
    pub(crate) fn wear_held(&self, player: PlayerId, on: WearOn, proc_id: Option<&str>) {
        let Some(held) = player_held(player) else {
            return;
        };
        let Some((tool_slots, stats)) = self.tools.get(&held.item) else {
            return;
        };
        let Some(mut rec) = Record::of_stack(&held.data) else {
            return;
        };
        let mut worn = false;
        for socket in 0..rec.entries.len() {
            let e = &rec.entries[socket];
            if e.id.is_empty() || e.cond == 0 {
                continue;
            }
            let Some(w) = self.fit_of(&e.id, &stats.kind).and_then(|f| f.wear) else {
                continue;
            };
            let qualifies = w.on == on || (w.on == WearOn::Proc && proc_id == Some(e.id.as_str()));
            if !qualifies {
                continue;
            }
            if rng_u64(WEAR_STREAM) % (w.max as u64) < 100 {
                rec.entries[socket].cond -= 1;
                worn = true;
            }
        }
        if !worn {
            return;
        }
        if let Some(stamped) = self.stamp(&held, tool_slots, stats, &rec) {
            let expect: Vec<(&str, &[u8])> = held
                .data
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_slice()))
                .collect();
            let data: Vec<(&str, &[u8])> = stamped
                .data
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_slice()))
                .collect();
            set_player_held_data(player, &held.item, &expect, &data);
        }
    }

    /// The occupied-socket gestures, committed on the drop like carving:
    /// the identity's own material REPAIRS its augment at the install rate
    /// (consuming only what the bar needs), a socket material UPGRADES the
    /// mount one level per gem up to [`LEVEL_MAX`]. Anything the gestures
    /// do not consume is left for the workstation sweep.
    ///
    /// Returns whether a mount UPGRADE committed, which is the caller's cue
    /// to sound the gem gesture — repair is silent, and the answer is only
    /// ever true once the restamp has actually landed on the tool.
    pub(super) fn tend_sockets(&self, slots: &mut [Option<ItemStackData>]) -> bool {
        let (tool, mut rec) = {
            let Some((tool, _, _, rec)) = self.tool_in(slots) else {
                return false;
            };
            (tool.clone(), rec)
        };
        let Some((tool_slots, stats)) = self.tools.get(&tool.item) else {
            return false;
        };
        let mut consumed: Vec<(usize, u8)> = Vec::new();
        let mut upgraded = false;
        for (cell, slot) in slots.iter().enumerate().take(SLOTS).skip(1) {
            let socket = cell - 1;
            let Some(stack) = slot.as_ref().filter(|s| s.count > 0) else {
                continue;
            };
            let Some(entry) = rec.entry_at(socket) else {
                continue;
            };
            let (id, cond, lvl) = (entry.id.clone(), entry.cond, entry.lvl);
            if self.socket_items.contains(&stack.item) {
                let n = (LEVEL_MAX - lvl).min(stack.count);
                if n > 0 {
                    rec.entry_mut(socket).lvl = lvl + n;
                    consumed.push((cell, n));
                    upgraded = true;
                }
            } else if self.material_of.get(&id) == Some(&stack.item) {
                if !repairable(cond, lvl) {
                    continue;
                }
                let Some(fit) = self.fit_of(&id, &stats.kind) else {
                    continue;
                };
                let per = Self::repair_quanta(fit);
                let need = quanta_max(lvl).saturating_sub(cond);
                let n = (need.div_ceil(per)).min(stack.count as u16) as u8;
                if n > 0 {
                    rec.entry_mut(socket).cond = (cond + n as u16 * per).min(quanta_max(lvl));
                    consumed.push((cell, n));
                }
            }
        }
        if consumed.is_empty() {
            return false;
        }
        let Some(stamped) = self.stamp(&tool, tool_slots, stats, &rec) else {
            return false;
        };
        slots[SLOT_TOOL] = Some(stamped);
        for (cell, n) in consumed {
            take(&mut slots[cell], n);
        }
        upgraded
    }

    /// The staged fits: every socket cell holding a material with a valid
    /// fit for this tool right now — `(cell, fit, count in the cell)`.
    /// Valid means: the cell is OPEN and free, the identity is nowhere on
    /// the record (the same augment type never repeats), and the fit does
    /// not grant a behaviour the tool's row already has innately.
    ///
    /// One identity stages at most ONCE however many cells offer it, and
    /// the cell that can AFFORD it wins: an unaffordable cell must never
    /// shadow a stocked one, or the button sits disabled beside enough
    /// material. When no cell can afford it the FIRST keeps the slot, so
    /// the panel still hints what it is short of.
    ///
    /// The result is in CELL order. Resolving affordability can move an
    /// identity to a later cell than the one that first claimed it, and the
    /// panel composites its preview layers in this order while the commit
    /// writes them positionally — so without the sort the preview could
    /// stack two augments in an order the apply would not reproduce.
    pub(super) fn staged<'a>(
        &'a self,
        slots: &'a [Option<ItemStackData>],
    ) -> Vec<(usize, &'a Fit, u8)> {
        let Some((tool, tool_slots, stats, rec)) = self.tool_in(slots) else {
            return Vec::new();
        };
        let installed: HashSet<&str> = rec.installed().collect();
        let mut out: Vec<(usize, &Fit, u8)> = Vec::new();
        for (cell, slot) in slots.iter().enumerate().take(SLOTS).skip(1) {
            let socket = cell - 1;
            if !matches!(Self::cell_state(tool_slots, &rec, socket), CellState::Open) {
                continue;
            }
            let Some(material) = slot.as_ref().filter(|s| s.count > 0) else {
                continue;
            };
            let Some(fits) = self.augments.get(&material.item) else {
                continue;
            };
            let fit = fits.iter().find(|f| {
                f.tool == stats.kind
                    && !installed.contains(f.overlay.as_str())
                    && !(f.gentle.is_some() && self.nondestructive.contains(&tool.item))
            });
            let Some(fit) = fit else {
                continue;
            };
            let staged_at = out.iter().position(|(_, f, _)| f.overlay == fit.overlay);
            match staged_at {
                None => out.push((cell, fit, material.count)),
                Some(i) if out[i].2 < out[i].1.cost && material.count >= fit.cost => {
                    out[i] = (cell, fit, material.count);
                }
                Some(_) => {}
            }
        }
        out.sort_by_key(|(cell, _, _)| *cell);
        out
    }

    /// The tool with every AFFORDABLE staged fit applied, plus the per-cell
    /// costs to consume — `None` when nothing affordable is staged or the
    /// stamped record would cross the engine's value cap.
    pub(super) fn apply_staged(
        &self,
        slots: &[Option<ItemStackData>],
    ) -> Option<(ItemStackData, Vec<(usize, u8)>)> {
        let (tool, tool_slots, stats, rec) = self.tool_in(slots)?;
        let mut rec = rec;
        let mut consumes = Vec::new();
        for (cell, fit, have) in self.staged(slots) {
            if have < fit.cost {
                continue;
            }
            rec.set_id(cell - 1, &fit.overlay);
            consumes.push((cell, fit.cost));
        }
        if consumes.is_empty() {
            return None;
        }
        let stamped = self.stamp(tool, tool_slots, stats, &rec)?;
        Some((stamped, consumes))
    }

    /// Stamp `rec` onto the tool: the record itself, and the engine override
    /// plus overlay art RECOMPUTED from the base row and the full record,
    /// never incrementally from the previous stamp. `None` when a recorded
    /// identity has no fit for this tool's kind (a richer pack set wrote
    /// it), or a stamped value would cross the engine's cap.
    pub(super) fn stamp(
        &self,
        tool: &ItemStackData,
        tool_slots: &ToolSlots,
        stats: &ToolStats,
        rec: &Record,
    ) -> Option<ItemStackData> {
        let fitted: Vec<(&Entry, &Fit)> = rec
            .entries
            .iter()
            .filter(|e| !e.id.is_empty())
            .map(|e| self.fit_of(&e.id, &stats.kind).map(|f| (e, f)))
            .collect::<Option<_>>()?;

        let record = rec.encode();
        if record.len() > VALUE_CAP {
            return None;
        }
        let mut data = tool.data.clone();
        data.retain(|(k, _)| k != AUGMENTS_KEY && k != TOOL_OVERRIDE_KEY && k != OVERLAY_DATA_KEY);
        data.push((AUGMENTS_KEY.to_owned(), record.into_bytes()));

        // A carved-but-unaugmented tool carries only the record: the engine
        // override and the art exist exactly when augments do.
        if !fitted.is_empty() {
            // A BROKEN augment (condition 0) contributes NO stats — the tool
            // falls back toward its row until repaired — but keeps its
            // identity (the socket stays occupied) and its art.
            let live = || fitted.iter().filter(|(e, _)| e.cond > 0).map(|(_, f)| f);
            let tier = live().fold(stats.tier, |t, f| t.max(f.tier));
            let speed = live().fold(stats.speed, |s, f| s * f.speed_mult);
            let damage = live().fold(stats.damage, |d, f| {
                [d[0] * f.damage_mult, d[1] * f.damage_mult]
            });
            // The IDENTITIES recorded are the fits' canonical overlays; the
            // ART drawn is the family-resolved list, so a stone pickaxe's tip
            // hugs the stone silhouette while both record "forge:diamond_tip".
            let arts = fitted
                .iter()
                .map(|(_, f)| f.overlay_for(&tool_slots.family))
                .collect::<Vec<_>>()
                .join(",");
            if arts.len() > VALUE_CAP {
                return None;
            }
            data.push((
                TOOL_OVERRIDE_KEY.to_owned(),
                tool_override_json(tier, speed, damage).into_bytes(),
            ));
            data.push((OVERLAY_DATA_KEY.to_owned(), arts.into_bytes()));
        }
        data.sort_by(|a, b| a.0.cmp(&b.0));
        Some(ItemStackData {
            data,
            ..tool.clone()
        })
    }
}
