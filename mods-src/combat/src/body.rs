//! The pack's per-body pass: every claim this pack states about one body,
//! resolved and written TOGETHER.
//!
//! One mod holds one claim slot per seam, so the last write of each seam
//! must already be the merged answer — the guard's stance and the swing
//! clock's pose publishing in sequence would have the second clobber the
//! first. This module is where the pack's features meet: [`run`] steps
//! every rule's clock, folds the rules' [`Claims`] in list order, runs the
//! swing law's clock as one more set of claims, and [`publish`] writes
//! each seam ONCE — and only when the merged answer changed, since the
//! engine keeps a claim until its claimant re-states it. Both sides run
//! it — the server tick for every body, the client frame for the local
//! one.
//!
//! It also owns the TOOL TABLE: which items this pack animates, each
//! filed under its family's row, resolved once at init; and the CLOCKS one
//! body runs, kept as one struct per body so a tick costs one lookup per
//! player.

use crate::bow;
use crate::claims::{self, Claims, Rule};
use crate::families::{Families, Family, Style, FAMILIES_JSON};
use crate::guard;
use crate::strike::Profile;
use crate::swing;
use mod_sdk::*;

/// One tool this pack animates: a registry row whose tool `kind` named one
/// of the families.
struct Tool {
    id: ItemId,
    style: Style,
}

/// One fixed tick's seconds — the SERVER clocks' step (20 TPS). The
/// client's clocks step on the frame clock; both run the same laws, and
/// the engine's eased pose lane makes the two rates one motion.
pub const TICK_SECONDS: f32 = 1.0 / 20.0;

/// Everything one body carries between steps: which rule holds its press,
/// every clock the rules and the swing law run, and the claims last
/// published for it. Both sides keep one per body they animate — the
/// server for every player, a client for the local one — stepped on the
/// tick and the frame alike; only the server acts on their edges.
#[derive(Default)]
pub struct BodyClocks {
    /// The rule (its index in the pack's list) that took the current use
    /// press, recorded when it was pressed. Meaningful only while the
    /// snapshot's `holds_use` is up.
    pub press_owner: Option<usize>,
    pub swing: swing::Clock,
    pub draw: bow::Clock,
    pub recoil: guard::Recoil,
    /// The claims [`publish`] last wrote for this body; `None` before the
    /// first write.
    last: Option<Claims>,
}

/// The tool table: which items this pack animates.
#[derive(Default)]
pub struct Tools {
    families: Families,
    /// The tools this pack animates, swept from the registry once at init.
    /// A family with no rows in this build is one row the pack never
    /// runs, never a dead pack.
    tools: Vec<Tool>,
}

impl Tools {
    /// Build the table off the registry's tool rows: every item whose tool
    /// data names a `kind` this pack's families cover joins that family,
    /// whichever pack registered it and whatever its tier — the family is
    /// a fact of the row, never a list kept here. Registry-only, legal on
    /// every instance. (The row's tool data and the per-stack override
    /// share the engine's one tool key; this reads the row side.)
    pub fn resolve() -> Tools {
        let (mut families, refused) = Families::parse(FAMILIES_JSON);
        for kind in refused {
            log(&format!(
                "[combat] the `{kind}` family row is incomplete — it is refused"
            ));
        }
        for kind in families.resolve_impacts(animation_clip) {
            log(&format!(
                "[combat] not every `{kind}` attack clip marks an impact — its hits stay the engine's"
            ));
        }
        let mut tools = Vec::new();
        for (id, text) in items_with_data(TOOL_OVERRIDE_KEY) {
            let kind = json::Value::parse(&text)
                .and_then(|row| row.get("kind")?.as_str().map(str::to_owned));
            let Some(kind) = kind else {
                log(&format!(
                    "[combat] a tool row's data names no kind — its swings stay vanilla: {text}"
                ));
                continue;
            };
            // Shovels and shears are tools too; they are simply not this
            // pack's to swing.
            let Some(style) = families.of_kind(&kind) else {
                continue;
            };
            tools.push(Tool { id, style });
        }
        for style in families.styles() {
            if !tools.iter().any(|t| t.style == style) {
                log(&format!(
                    "[combat] this registry has no `{}` rows — that family never swings",
                    families.get(style).kind
                ));
            }
        }
        Tools { families, tools }
    }

    /// Whether the table is empty — a build whose registry carries none of
    /// the tools, which leaves this whole half of the pack inert.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Whether a hand holding `held` is one this pack paces — the combo
    /// handler's question about an attacker.
    pub fn paces(&self, held: Option<ItemId>) -> bool {
        self.family_of(held).is_some()
    }

    /// Whether a hand holding `held` LANDS ITS OWN HITS: a paced tool whose
    /// every attack clip marks an impact — the attack-attempt handler's
    /// question, since claiming a press is a promise to land it.
    pub fn lands(&self, held: Option<ItemId>) -> bool {
        self.family_of(held)
            .is_some_and(|(_, family)| !family.impacts.is_empty())
    }

    /// How `style`'s swing reaches, for the strike law.
    pub fn profile(&self, style: Style) -> &Profile {
        &self.families.get(style).profile
    }

    /// Which family `held` swings, if it is one of this pack's tools.
    fn family_of(&self, held: Option<ItemId>) -> Option<(Style, &Family)> {
        let tool = self.tools.iter().find(|t| Some(t.id) == held)?;
        Some((tool.style, self.families.get(tool.style)))
    }

    /// The swing law's claims on one body this step, and — on the
    /// authority — the family whose attack LANDED on this step (the clock
    /// crossed the step's impact): landing a hit is the server's to do.
    /// `holds_press` is whether a rule holds the use press: the swing claim
    /// yields to it.
    fn swing(
        &self,
        clock: &mut swing::Clock,
        state: &PlayerSnapshot,
        holds_press: bool,
        authority: bool,
        dt: f32,
    ) -> (Claims, Option<Style>) {
        // The swing claim rests with the TOOL, idle hands included — a held
        // pickaxe never silently regains the vanilla swing between swings —
        // and yields to a rule holding the press (a raised guard, a drawn
        // bow), whose own stance law owns those hands while its denial
        // keeps them still. Only the Swing motion: the jab stays the
        // engine's, so a tool interacts like any item.
        let family = self.family_of(state.held);
        let style = family.map(|(style, _)| style);
        let claimed = swing::claim(style, holds_press);

        // The clock runs only a claimed hand; an unclaimed style resets it,
        // so lowering the guard never resumes a swing frozen mid-arc.
        let played = clock.step(
            family.filter(|_| claimed),
            state.swing.main,
            state.swing.mining,
            dt,
        );
        let landed = (authority && clock.impact()).then_some(style).flatten();
        let plays = played
            .zip(family)
            .map(|(play, (_, family))| swing::plays(family, play, clock.attacking()))
            .unwrap_or_default();

        // While this pack's clock paces a claimed hand, ITS clock is the
        // attack rate: the engine cooldown stands negated, and the arc
        // bars the next attack (a denial) until its recovery — the
        // animation and the pace cannot disagree, because they are one
        // clock. An unclaimed hand releases the scale and the engine's own
        // cooldown returns. A tool that LANDS its own hits keeps the press
        // flowing instead: the clock hears a mid-arc click and queues it
        // (the engine's melee is already stood down by the attack-attempt
        // claim), so the denial is only for a paced tool the engine still
        // hits for.
        let denied = if clock.bars_attack() && !self.lands(state.held) {
            vec![BodyAction::Attack]
        } else {
            Vec::new()
        };
        let claims = Claims {
            cooldown: if claimed { 0.0 } else { 1.0 },
            params: if claimed {
                claims::swing_claim(0)
            } else {
                Vec::new()
            },
            denied,
            // A settled swing releases the slots, which fade back into the
            // hand's own animation rather than popping; the body's clips
            // are masked over the walk, so a swing strikes without freezing
            // the stride.
            plays,
            ..Default::default()
        };
        (claims, landed)
    }
}

/// One body's whole pass, shared by both sides (`authority` = the server,
/// which alone acts on edges and makes the simulation claims); `dt` is
/// the caller's clock step in seconds. Steps the recoil, every rule's
/// clock and the swing's, composes every claim, publishes each seam once
/// if anything changed, and answers the family whose attack LANDED on this
/// step, for the caller to strike with.
pub fn run(
    tools: &Tools,
    rules: &[Box<dyn Rule>],
    player: PlayerId,
    clocks: &mut BodyClocks,
    state: &PlayerSnapshot,
    authority: bool,
    dt: f32,
) -> Option<Style> {
    let dt_ticks = dt / TICK_SECONDS;
    clocks.recoil.step(dt_ticks);
    // Every rule's clock steps FIRST: the claims read this step's state,
    // and an edge (the draw coming off) is acted on before the body is
    // published at rest.
    for (index, rule) in rules.iter().enumerate() {
        let press = claims::presses(clocks, state, index);
        rule.step(clocks, player, state, press, dt_ticks, authority);
    }
    let claims = claims::compose(rules, state, clocks);
    // The swing's claims compose UNDER the rules': a rule holding the press
    // stands the swing down, so the two never pose one hand at once.
    let (swing, landed) = tools.swing(&mut clocks.swing, state, claims.holds_press, authority, dt);
    let claims = claims.over(swing);
    if clocks.last.as_ref() != Some(&claims) {
        publish(player, claims.clone(), authority);
        clocks.last = Some(claims);
    }
    landed
}

/// Write the composed claims, ONE call per seam. The simulation claims
/// (speed, cooldown, denials) are the server's alone: a client predicting
/// them would argue with the validator.
fn publish(player: PlayerId, claims: Claims, authority: bool) {
    if authority {
        set_player_attribute(player, PlayerAttribute::MoveSpeed, claims.speed);
        set_player_attribute(player, PlayerAttribute::AttackCooldown, claims.cooldown);
        set_player_denied_actions(player, claims.denied);
    }
    let [display_main, display_off] = &claims.display;
    set_player_held_display(player, display_main.as_deref(), display_off.as_deref());
    set_player_held_pose(player, claims.main, claims.off);
    set_player_bone_pose(player, claims.bones);
    set_player_animator_params(player, claims.params);
    set_player_animator_plays(player, claims.plays);
}
