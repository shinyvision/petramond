//! The tool FAMILIES this pack animates — one family per tool `kind`: the
//! clips each combo step and the work loop play on each rig, the pacing, and
//! the strike profile. A family is ROW DATA: the [`FAMILY_DATA`](crate::keys::FAMILY_DATA) entry on an
//! item row declares the family of that row's own tool kind (this pack
//! patches its three onto a stone pickaxe, a stone axe and its iron sword),
//! so another pack adds a weapon family by carrying the entry on one of its
//! own tool rows — no code change, no rebuild of this mod.
//!
//! What the data cannot state is read off the rigs after parsing
//! ([`Families::resolve_impacts`]): where each attack clip marks its
//! `impact`, on the body rig (the instant the hit lands) and on the
//! first-person rig (the frame the wielder sees it land).

use serde::Deserialize;

use crate::strike::{Profile, ProfileSpec};
use mod_sdk::*;

/// The [`FAMILY_DATA`](crate::keys::FAMILY_DATA) entry, as a pack writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FamilySpec {
    /// The attack combo in chain order; never empty.
    attacks: Vec<MotionSpec>,
    work: MotionSpec,
    pace: PaceSpec,
    profile: ProfileSpec,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionSpec {
    fp: String,
    body: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaceSpec {
    window_attack: Vec<f32>,
    window_mine: f32,
}

impl MotionSpec {
    fn motion(self) -> Motion {
        Motion {
            first_person: self.fp,
            body: self.body,
        }
    }
}

impl FamilySpec {
    /// Every clip the family plays, as (first-person, body) pairs.
    #[cfg(test)]
    pub fn clips(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attacks
            .iter()
            .chain([&self.work])
            .map(|m| (m.fp.as_str(), m.body.as_str()))
    }
}

/// A family, as its index in the table.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub struct Style(pub usize);

/// The clips one motion plays: the first-person and the body rig's.
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub first_person: String,
    pub body: String,
}

/// A family's pacing.
#[derive(Clone, Debug, PartialEq)]
pub struct Pace {
    /// Per-combo-step ATTACK windows in seconds, positional with the
    /// attacks (wrapping) — the arc IS the weapon's rate.
    pub attack: Vec<f32>,
    /// The WORK window: the mining loop and its break impacts. The engine
    /// retriggers its dig thunk every 0.300 s while mining, so a work
    /// window near that lands the impact frame on the sound.
    pub mine: f32,
}

/// One family.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub kind: String,
    /// The attack combo in chain order, wrapping; never empty.
    pub attacks: Vec<Motion>,
    pub work: Motion,
    pub pace: Pace,
    pub profile: Profile,
    /// Per-step IMPACT phases of the BODY clips — the rig every observer
    /// sees, so the hit and the drawn strike are one instant on every
    /// mirror. Non-empty only when EVERY step marks one: a family whose
    /// steps disagreed about whether the swing lands its own hit would
    /// land some attacks at the click and others at the arc, so it is all
    /// or nothing, and empty leaves the family's hits to the engine's
    /// crosshair melee.
    pub impacts: Vec<f32>,
    /// Per-step impact phases of the FIRST-PERSON clips, positional; a
    /// step whose viewmodel clip marks none scrubs at the body's phase.
    pub fp_impacts: Vec<Option<f32>>,
}

impl Family {
    /// The attack window of combo step `combo`.
    pub fn attack_window(&self, combo: usize) -> f32 {
        self.pace.attack[combo % self.pace.attack.len()]
    }

    /// Where step `combo`'s body clip lands, as a phase; `None` when the
    /// family lands nothing of its own.
    pub fn impact_phase(&self, combo: usize) -> Option<f32> {
        (!self.impacts.is_empty()).then(|| self.impacts[combo % self.impacts.len()])
    }
}

/// The table, in declaration order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Families {
    rows: Vec<Family>,
}

impl Families {
    /// Build the table from each kind's declared family, in the order given.
    /// A family whose data breaks a rule serde cannot state (an empty combo,
    /// a non-positive window) or that repeats a kind already declared is
    /// refused whole — its kind then swings vanilla, never half a family —
    /// and answered, with the reason, beside the table for the caller to log.
    pub fn from_specs(
        specs: impl IntoIterator<Item = (String, FamilySpec)>,
    ) -> (Families, Vec<(String, String)>) {
        let mut rows: Vec<Family> = Vec::new();
        let mut refused = Vec::new();
        for (kind, spec) in specs {
            if rows.iter().any(|f| f.kind == kind) {
                refused.push((kind, "the kind already has a family".to_owned()));
                continue;
            }
            match Family::from_spec(&kind, spec) {
                Ok(family) => rows.push(family),
                Err(reason) => refused.push((kind, reason)),
            }
        }
        (Families { rows }, refused)
    }

    /// Read every attack clip's `impact` marker off the rigs through
    /// `clip_info` (the host's `animation_clip`). Answers the kinds whose
    /// body clips do not all mark one — those families land nothing of
    /// their own.
    pub fn resolve_impacts(
        &mut self,
        clip_info: impl Fn(&str, &str) -> Option<AnimationClipInfo>,
    ) -> Vec<String> {
        let phase = |rig: &str, clip: &str| -> Option<f32> {
            let info = clip_info(rig, clip)?;
            let (_, at) = info.markers.iter().find(|(name, _)| name == "impact")?;
            (info.length > 0.0).then(|| at / info.length)
        };
        let mut unlanded = Vec::new();
        for family in &mut self.rows {
            let body: Option<Vec<f32>> = family
                .attacks
                .iter()
                .map(|m| phase(rig::PLAYER_BODY, &m.body))
                .collect();
            family.impacts = body.unwrap_or_else(|| {
                unlanded.push(family.kind.clone());
                Vec::new()
            });
            family.fp_impacts = family
                .attacks
                .iter()
                .map(|m| phase(rig::PLAYER_FIRST_PERSON, &m.first_person))
                .collect();
        }
        unlanded
    }

    pub fn styles(&self) -> impl Iterator<Item = Style> {
        (0..self.rows.len()).map(Style)
    }

    /// The family a tool row's `kind` names — the engine's own tool
    /// vocabulary, so any pack's tool of any tier swings here.
    pub fn of_kind(&self, kind: &str) -> Option<Style> {
        self.rows.iter().position(|f| f.kind == kind).map(Style)
    }

    pub fn get(&self, style: Style) -> &Family {
        &self.rows[style.0]
    }
}

impl Family {
    fn from_spec(kind: &str, spec: FamilySpec) -> Result<Family, String> {
        if spec.attacks.is_empty() {
            return Err("the attack combo is empty".into());
        }
        let PaceSpec {
            window_attack: attack,
            window_mine: mine,
        } = spec.pace;
        if attack.is_empty() || attack.iter().any(|w| *w <= 0.0) {
            return Err("every attack window must be positive".into());
        }
        if mine <= 0.0 {
            return Err("the mining window must be positive".into());
        }
        Ok(Family {
            kind: kind.to_owned(),
            attacks: spec.attacks.into_iter().map(MotionSpec::motion).collect(),
            work: spec.work.motion(),
            pace: Pace { attack, mine },
            profile: Profile::from_spec(&spec.profile),
            impacts: Vec::new(),
            fp_impacts: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = r#"{
        "attacks": [{"fp": "m:fp_a", "body": "m:body_a"}, {"fp": "m:fp_b", "body": "m:body_b"}],
        "work": {"fp": "m:fp_work", "body": "m:body_work"},
        "pace": {"window_attack": [0.5, 0.25], "window_mine": 0.4},
        "profile": {"reach": 3, "sweet": 2, "arc_yaw": 30, "arc_pitch": 20, "peak": 1.5, "floor": 0.5, "cleave": true}
    }"#;

    fn info(length: f32, impact: Option<f32>) -> AnimationClipInfo {
        AnimationClipInfo {
            length,
            looping: false,
            markers: impact
                .map(|at| ("impact".to_string(), at))
                .into_iter()
                .collect(),
        }
    }

    fn spec(text: &str) -> FamilySpec {
        parse_row_data(text).unwrap_or_else(|e| panic!("{e}"))
    }

    /// A family only WHOLE: data missing a field does not parse at all, one
    /// that breaks a rule or repeats a kind is refused alone, the table keys
    /// `of_kind` in declaration order, and the impacts are read off the rigs
    /// per step — the body's all or nothing, the viewmodel's positional.
    #[test]
    fn families_are_whole_and_impacts_come_off_the_rigs() {
        let no_work = ROW.replace(r#""work": {"fp": "m:fp_work", "body": "m:body_work"},"#, "");
        assert!(parse_row_data::<FamilySpec>(&no_work).is_err());
        let bare = ROW.replace(
            r#"[{"fp": "m:fp_a", "body": "m:body_a"}, {"fp": "m:fp_b", "body": "m:body_b"}]"#,
            "[]",
        );
        let (mut families, refused) = Families::from_specs([
            ("hammer".to_owned(), spec(ROW)),
            ("bare".to_owned(), spec(&bare)),
            ("hammer".to_owned(), spec(ROW)),
        ]);
        assert_eq!(families.styles().count(), 1, "the broken ones are refused alone");
        let refused: Vec<&str> = refused.iter().map(|(kind, _)| kind.as_str()).collect();
        assert_eq!(refused, ["bare", "hammer"]);
        let hammer = families
            .of_kind("hammer")
            .expect("the whole row is a family");
        assert_eq!(families.of_kind("bare"), None);
        assert_eq!(Families::from_specs([]).0.styles().count(), 0);

        let family = families.get(hammer);
        assert_eq!(
            family.attack_window(3),
            0.25,
            "windows are positional and wrap"
        );
        assert_eq!(family.impact_phase(0), None, "nothing resolved yet");
        assert!(family.profile.cleave);

        let unlanded = families.resolve_impacts(|rig, clip| match (rig, clip) {
            (rig::PLAYER_BODY, "m:body_a") => Some(info(2.0, Some(1.0))),
            (rig::PLAYER_BODY, "m:body_b") => Some(info(1.0, Some(0.2))),
            (rig::PLAYER_FIRST_PERSON, "m:fp_a") => Some(info(1.0, Some(0.6))),
            (rig::PLAYER_FIRST_PERSON, "m:fp_b") => Some(info(1.0, None)),
            _ => None,
        });
        assert!(unlanded.is_empty());
        let family = families.get(hammer);
        assert_eq!(family.impacts, [0.5, 0.2], "phases, not seconds");
        assert_eq!(family.fp_impacts, [Some(0.6), None]);
        assert_eq!(family.impact_phase(2), Some(0.5), "wraps over the combo");

        // One step without a body marker: the whole family lands nothing.
        let unlanded = families.resolve_impacts(|rig, clip| match (rig, clip) {
            (rig::PLAYER_BODY, "m:body_a") => Some(info(2.0, Some(1.0))),
            _ => None,
        });
        assert_eq!(unlanded, ["hammer"]);
        assert!(families.get(hammer).impacts.is_empty());
    }
}
