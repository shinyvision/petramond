//! The pack's per-body publisher: every claim this pack states about one
//! body, resolved and written TOGETHER.
//!
//! One mod holds one claim slot per seam, so the last write of each seam
//! must already be the merged answer — the guard's stance and the swing
//! clock's pose publishing in sequence would have the second clobber the
//! first. This module is where the pack's features meet: each contributes
//! its half (the guard law hands its resolved [`Guard`] in, the swing law's
//! clock runs here), and [`Bodies::publish`] writes each seam once.
//!
//! It also owns the TOOL TABLE: which items this pack animates, each tool's
//! harness-authored curves and windows, resolved once at init.

use crate::guard::Guard;
use crate::swing;
use mod_sdk::animation::{BodyCurve, PoseCurve};
use mod_sdk::*;
use std::rc::Rc;

/// The axe's harness-authored swing COMBO, shipped verbatim from the
/// animation harness (`petramond-swing-animation`, v1), in chain order: the
/// first entry is the opening swing AND the mining loop's repeat; each entry
/// after it is what the next quick follow-up attack plays (wrapping).
/// Parsed once at init; a combo that does not fully parse is refused whole
/// and the compiled chop stands in.
const AXE_COMBO_JSON: &[&str] = &[
    include_str!("../swings/axe.chop.json"),
    include_str!("../swings/axe.chop2.json"),
    include_str!("../swings/axe.chop3.json"),
];

/// The pickaxe's harness-authored swing — one step, so it is the mining
/// loop's repeat AND every attack; further chain steps are one
/// `include_str!` each when their exports land.
const PICKAXE_COMBO_JSON: &[&str] = &[include_str!("../swings/pickaxe.strike.json")];

/// The families' harness-authored BODY animations
/// (`petramond-player-animation`, v1): the third-person arm choreography
/// every swing of a family plays, shipped verbatim from the
/// player-animation harness. One per family — the item combo alternates
/// steps, the body plays its one choreography throughout.
const PICKAXE_PLAYER_JSON: &str = include_str!("../swings/pickaxe.player.json");
const AXE_PLAYER_JSON: &str = include_str!("../swings/axe.player.json");

/// One tool this pack animates: its resolved item, its curve family, and —
/// when the pack ships harness curves for the family — its attack combo, in
/// chain order.
#[derive(Clone)]
struct Tool {
    id: ItemId,
    style: swing::Style,
    /// Empty = no data shipped; the compiled family curve plays every swing.
    combo: Rc<[PoseCurve]>,
    /// Per-combo-step ATTACK windows, positional with `combo`: each item
    /// export's authored `window_attack` (the default where a file carries
    /// none). Empty when no data shipped — every attack plays the default.
    attack_windows: Rc<[f32]>,
    /// The WORK window — the mining loop and its break impacts — from the
    /// first export's `window_mine` (mining always plays step 0).
    mine_window: f32,
    /// Per-combo-step IMPACT phases, positional with `combo`: each export's
    /// flagged key. Non-empty only when EVERY step marks one — a family
    /// whose steps disagreed about whether the swing lands its own hit
    /// would land some attacks at the click and others at the arc, so it is
    /// all or nothing: empty leaves the family's hits to the engine's
    /// crosshair melee, exactly like a family that ships no data.
    impacts: Rc<[f32]>,
    /// The family's authored third-person choreography; `None` = the
    /// compiled arm columns stand in.
    body: Option<Rc<BodyCurve>>,
}

impl Tool {
    /// The harness curve for one play, when this tool ships any. The chain
    /// position picks the combo step, wrapping, so a combo of any length
    /// alternates forever. (Every play IS the tool's own swing — the clock
    /// animates nothing else; use jabs stay the engine's.)
    fn curve_for(&self, combo: usize) -> Option<&PoseCurve> {
        if self.combo.is_empty() {
            return None;
        }
        Some(&self.combo[combo % self.combo.len()])
    }
}

/// The tool table and the swing clocks — everything the per-body publish
/// reads and advances.
#[derive(Default)]
pub struct Bodies {
    /// The tools this pack animates, resolved once at init. A missing row (a
    /// tier not in this build's registry) is one disabled curve, never a
    /// dead pack.
    tools: Vec<Tool>,
    /// SERVER: one swing clock per body, pruned against the roster each
    /// pass ([`Bodies::prune`]), so a leaver's slot dies with their session.
    swings: Vec<(PlayerId, swing::Clock)>,
    /// CLIENT: the same clock, local player only.
    local: swing::Clock,
}

impl Bodies {
    /// Resolve the tool rows. Registry-only, legal on every instance; a row
    /// that does not resolve is one curve the pack will never run, so it
    /// logs loudly rather than dying silently.
    pub fn resolve(&mut self) {
        for (names, style) in [
            (swing::PICKAXES, swing::Style::Pickaxe),
            (swing::AXES, swing::Style::Axe),
        ] {
            // A family's harness combo parses ONCE, WHOLE: the entries are
            // positional (the first is also the mining loop's repeat, the
            // rest are the chain), so one broken file refuses the whole set
            // — loudly — and the compiled family curve stands in. Never half
            // a chain, and never a chop2 promoted into the mining loop.
            let sources: &[&str] = match style {
                swing::Style::Pickaxe => PICKAXE_COMBO_JSON,
                swing::Style::Axe => AXE_COMBO_JSON,
            };
            let combo: Rc<[PoseCurve]> = match sources
                .iter()
                .map(|text| PoseCurve::from_harness(text))
                .collect::<Option<Vec<_>>>()
            {
                Some(curves) => curves.into(),
                None => {
                    log(&format!(
                        "[combat] a {style:?} swing export did not parse — the compiled curve stands in"
                    ));
                    Vec::new().into()
                }
            };
            let attack_windows: Rc<[f32]> = combo
                .iter()
                .map(|c| c.window_attack().unwrap_or(swing::ATTACK_SECONDS))
                .collect();
            let mine_window = combo
                .first()
                .and_then(|c| c.window_mine())
                .unwrap_or(swing::MINE_SECONDS);
            let impacts: Rc<[f32]> = match combo
                .iter()
                .map(|c| c.impact())
                .collect::<Option<Vec<_>>>()
            {
                Some(phases) if !phases.is_empty() => phases.into(),
                _ => {
                    if !combo.is_empty() {
                        log(&format!(
                            "[combat] not every {style:?} export marks an impact — its hits stay the engine's"
                        ));
                    }
                    Vec::new().into()
                }
            };
            // The family's body export parses by the same whole-or-nothing
            // rule; a refused file leaves the compiled arm playing.
            let player: Option<&str> = match style {
                swing::Style::Pickaxe => Some(PICKAXE_PLAYER_JSON),
                swing::Style::Axe => Some(AXE_PLAYER_JSON),
            };
            let body: Option<Rc<BodyCurve>> = player.and_then(|text| {
                let parsed = BodyCurve::from_harness(text);
                if parsed.is_none() {
                    log(&format!(
                        "[combat] the {style:?} player-animation export did not parse — the compiled arm stands in"
                    ));
                }
                parsed.map(Rc::new)
            });
            for name in names {
                match resolve_item(name) {
                    Some(id) => self.tools.push(Tool {
                        id,
                        style,
                        combo: combo.clone(),
                        attack_windows: attack_windows.clone(),
                        mine_window,
                        impacts: impacts.clone(),
                        body: body.clone(),
                    }),
                    None => log(&format!(
                        "[combat] '{name}' did not resolve — that tool's swings stay vanilla"
                    )),
                }
            }
        }
    }

    /// Whether the table is empty — a build whose registry carries none of
    /// the tools, which leaves this whole half of the pack inert.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Whether a hand holding `held` is one this pack paces — the combo
    /// handler's question about an attacker.
    pub fn paces(&self, held: Option<ItemId>) -> bool {
        self.tool_of(held).is_some()
    }

    /// Whether a hand holding `held` LANDS ITS OWN HITS: a paced tool whose
    /// every export marks an impact — the attack-attempt handler's question,
    /// since claiming a press is a promise to land it.
    pub fn lands(&self, held: Option<ItemId>) -> bool {
        self.tool_of(held).is_some_and(|t| !t.impacts.is_empty())
    }

    /// Drop the server clocks of every body not in `roster` — a leaver's
    /// clock dies with their session.
    pub fn prune(&mut self, roster: &[PlayerListEntry]) {
        self.swings
            .retain(|(id, _)| roster.iter().any(|entry| entry.id == *id));
    }

    /// Which of this pack's tools `held` names. OWNED (the curve is an `Rc`
    /// handle), so the row outlives no borrow and the mutable clock borrow
    /// in [`Bodies::publish`] never conflicts with it.
    fn tool_of(&self, held: Option<ItemId>) -> Option<Tool> {
        self.tools.iter().find(|t| Some(t.id) == held).cloned()
    }

    /// The swing clock stepping `player`'s body on this instance: the server
    /// keeps one per body; a client instance clocks only the local player.
    fn clock_of(&mut self, player: PlayerId, authority: bool) -> &mut swing::Clock {
        if !authority {
            return &mut self.local;
        }
        let at = match self.swings.iter().position(|(id, _)| *id == player) {
            Some(at) => at,
            None => {
                self.swings.push((player, swing::Clock::default()));
                self.swings.len() - 1
            }
        };
        &mut self.swings[at].1
    }

    /// One body's whole publish: the guard's claims and the swing clock's
    /// answer, merged into ONE write per seam. Shared by both sides
    /// (`authority` adds the claims only a server may make); `dt` is the
    /// caller's clock step. Answers the family whose attack LANDED on this
    /// step — the clock crossed the step's authored impact — on the
    /// authority side only: landing a hit is the server's to do.
    pub fn publish(
        &mut self,
        player: PlayerId,
        guard: &Guard,
        state: &PlayerSnapshot,
        authority: bool,
        dt: f32,
    ) -> Option<swing::Style> {
        // The swing claim rests with the TOOL, idle hands included — a held
        // pickaxe never silently regains the vanilla swing between swings —
        // and yields to a raised guard, whose own stance law owns those
        // hands while its denial keeps them still. Only the Swing motion:
        // the jab stays the engine's, so a tool interacts like any item.
        let tool = self.tool_of(state.held);
        let style = tool.as_ref().map(|t| t.style);
        let claimed = swing::claim(style, guard.raised);
        let motions = if claimed {
            vec![HandMotion::Swing]
        } else {
            vec![]
        };
        set_player_hand_motions(player, motions, vec![]);

        // The clock runs only a claimed hand; an unclaimed style resets it,
        // so lowering the guard never resumes a swing frozen mid-arc.
        let clock = self.clock_of(player, authority);
        let pace = tool
            .as_ref()
            .map(|t| swing::Pace {
                attack: &t.attack_windows,
                mine: t.mine_window,
                impact: &t.impacts,
            })
            .unwrap_or_default();
        let played = clock.step(
            style.filter(|_| claimed),
            state.swing.main,
            state.swing.mining,
            dt,
            pace,
        );
        let arc_bars_attack = clock.bars_attack();
        let landed = (authority && clock.impact()).then_some(style).flatten();
        let swung = played.map(|play| {
            let tool = tool.as_ref();
            let data = tool.and_then(|t| t.curve_for(play.combo));
            let body = tool.and_then(|t| t.body.as_deref());
            swing::pose(play.act, play.phase, data, body)
        });

        // How fast this body moves and what it may do are the server's to
        // resolve: a client predicting either would argue with the validator.
        if authority {
            set_player_attribute(player, PlayerAttribute::MoveSpeed, guard.speed_scale());
            // While this pack's clock paces a claimed hand, ITS clock is the
            // attack rate: the engine cooldown stands negated, and the arc
            // bars the next attack (a denial, below) until its recovery —
            // the animation and the pace cannot disagree, because they are
            // one clock. An unclaimed hand releases the scale and the
            // engine's own cooldown returns.
            let cooldown = if claimed { 0.0 } else { 1.0 };
            set_player_attribute(player, PlayerAttribute::AttackCooldown, cooldown);
            let mut denied = guard.denied();
            if arc_bars_attack {
                denied.push(BodyAction::Attack);
            }
            set_player_denied_actions(player, denied);
        }

        // The held pose, one call (one mod, one slot): a swinging tool owns
        // the main hand's item pose while its clock runs; the guard law
        // otherwise poses its own hand (the raised guard, or the lowered
        // carry). Never both — a hand holds one item. A settled swing
        // publishes `None`, which eases home: the item returns to its
        // authored hold, not a pop.
        let main = swung
            .as_ref()
            .map(|(pose, _)| *pose)
            .or_else(|| guard.pose(guard.main_holds));
        set_player_held_pose(player, main, guard.pose(guard.off_holds));

        // Bones likewise: a raised guard is a STANCE (Replace, holding every
        // joint it owns); a swing COMPOSES over the walk stride, because a
        // body that froze its stride to swing would stutter, not strike.
        let mut bones = guard.arms();
        if let Some((_, swing_bones)) = swung {
            bones.extend(swing_bones);
        }
        set_player_bone_pose(player, bones);
        landed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped combo parses WHOLE and its steps DIFFER: a broken
    /// export would silently demote the tool to its compiled curve, a chain
    /// whose follow-up replays the opening curve is a dead feature with no
    /// error anywhere, and a combo that half-parses would promote the wrong
    /// file into the mining loop.
    #[test]
    fn the_shipped_combos_parse_and_their_steps_differ() {
        for sources in [AXE_COMBO_JSON, PICKAXE_COMBO_JSON] {
            let combo: Vec<PoseCurve> = sources
                .iter()
                .map(|text| PoseCurve::from_harness(text).expect("every shipped step parses"))
                .collect();
            for a in 0..combo.len() {
                for b in a + 1..combo.len() {
                    assert_ne!(combo[a], combo[b], "steps {a} and {b} are one swing");
                }
            }
        }
        assert!(AXE_COMBO_JSON.len() >= 2, "the axe ships a chain");
        for text in [PICKAXE_PLAYER_JSON, AXE_PLAYER_JSON] {
            BodyCurve::from_harness(text).expect("every shipped body export parses");
        }
    }
}
