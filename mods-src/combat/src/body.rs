use crate::boomerang;
use crate::bow;
use crate::claims::{self, Claims, Rule};
use crate::families::{Families, Family, FamilySpec, Style};
use crate::guard;
use crate::keys::FAMILY_DATA;
use crate::strike::Profile;
use crate::swing;
use mod_sdk::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct ToolKindRow {
    kind: String,
}

struct Tool {
    id: ItemId,
    style: Style,
}

pub const TICK_SECONDS: f32 = 1.0 / 20.0;

#[derive(Default)]
pub struct BodyClocks {
    pub press_owner: Option<usize>,
    pub swing: swing::Clock,
    pub draw: bow::Clock,
    pub throwing: boomerang::motion::Clock,
    pub recoil: guard::Recoil,
    last: Option<Claims>,
}

#[derive(Default)]
pub struct Tools {
    families: Families,
    tools: Vec<Tool>,
}

impl Tools {
    pub fn resolve() -> Tools {
        let mut kinds: Vec<(ItemId, String)> = Vec::new();
        for (id, row) in items_with_data_as::<ToolKindRow>(TOOL_OVERRIDE_KEY) {
            kinds.push((id, row.kind));
        }
        let specs = items_with_data_as::<FamilySpec>(FAMILY_DATA)
            .into_iter()
            .filter_map(|(id, spec)| {
                let kind = kinds.iter().find(|(tool, _)| *tool == id).map(|(_, k)| k.clone());
                if kind.is_none() {
                    log(&format!(
                        "[combat] a '{FAMILY_DATA}' entry sits on a row with no tool kind — it is ignored"
                    ));
                }
                Some((kind?, spec))
            })
            .collect::<Vec<_>>();
        let (mut families, refused) = Families::from_specs(specs);
        for (kind, reason) in refused {
            log(&format!(
                "[combat] the `{kind}` family is refused: {reason}"
            ));
        }
        for kind in families.resolve_impacts(animation_clip) {
            log(&format!(
                "[combat] not every `{kind}` attack clip marks an impact — its hits stay the engine's"
            ));
        }
        let mut tools = Vec::new();
        for (id, kind) in kinds {
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

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn paces(&self, held: Option<ItemId>) -> bool {
        self.family_of(held).is_some()
    }

    pub fn lands(&self, held: Option<ItemId>) -> bool {
        self.family_of(held)
            .is_some_and(|(_, family)| !family.impacts.is_empty())
    }

    pub fn profile(&self, style: Style) -> &Profile {
        &self.families.get(style).profile
    }

    fn family_of(&self, held: Option<ItemId>) -> Option<(Style, &Family)> {
        let tool = self.tools.iter().find(|t| Some(t.id) == held)?;
        Some((tool.style, self.families.get(tool.style)))
    }

    fn swing(
        &self,
        clock: &mut swing::Clock,
        state: &PlayerSnapshot,
        holds_press: bool,
        authority: bool,
        dt: f32,
    ) -> (Claims, Option<Style>) {
        // Tool keeps the swing claim even idle, so a held pickaxe won't get the vanilla swing back
        // between swings.
        // A raised guard or drawn bow holding the press wins; its stance law owns the hands.
        // Only Swing, not the jab. The jab stays the engine's, so tools act like any other item.
        let family = self.family_of(state.held);
        let style = family.map(|(style, _)| style);
        let claimed = swing::claim(style, holds_press);

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

        // A claimed hand runs on this pack's clock: engine cooldown is negated and the arc
        // denies the next attack until recovery. Unclaimed, the engine cooldown comes back.
        // Tools that land their own hits queue a mid-arc click instead (engine melee is already
        // stood down by the attack-attempt claim).
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
            plays,
            ..Default::default()
        };
        (claims, landed)
    }
}

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
    for (index, rule) in rules.iter().enumerate() {
        let press = claims::presses(clocks, state, index);
        rule.step(clocks, player, state, press, dt_ticks, authority);
    }
    let claims = claims::compose(rules, state, clocks);
    let (swing, landed) = tools.swing(&mut clocks.swing, state, claims.holds_press, authority, dt);
    let claims = claims.over(swing);
    if clocks.last.as_ref() != Some(&claims) {
        publish(player, claims.clone(), authority);
        clocks.last = Some(claims);
    }
    landed
}

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
