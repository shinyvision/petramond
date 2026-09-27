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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FamilySpec {
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
    #[cfg(test)]
    pub fn clips(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attacks
            .iter()
            .chain([&self.work])
            .map(|m| (m.fp.as_str(), m.body.as_str()))
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub struct Style(pub usize);

#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub first_person: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pace {
    pub attack: Vec<f32>,
    pub mine: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub kind: String,
    pub attacks: Vec<Motion>,
    pub work: Motion,
    pub pace: Pace,
    pub profile: Profile,
    pub impacts: Vec<f32>,
    pub fp_impacts: Vec<Option<f32>>,
}

impl Family {
    pub fn attack_window(&self, combo: usize) -> f32 {
        self.pace.attack[combo % self.pace.attack.len()]
    }

    pub fn impact_phase(&self, combo: usize) -> Option<f32> {
        (!self.impacts.is_empty()).then(|| self.impacts[combo % self.impacts.len()])
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Families {
    rows: Vec<Family>,
}

impl Families {
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
        assert_eq!(
            families.styles().count(),
            1,
            "the broken ones are refused alone"
        );
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

        let unlanded = families.resolve_impacts(|rig, clip| match (rig, clip) {
            (rig::PLAYER_BODY, "m:body_a") => Some(info(2.0, Some(1.0))),
            _ => None,
        });
        assert_eq!(unlanded, ["hammer"]);
        assert!(families.get(hammer).impacts.is_empty());
    }
}
