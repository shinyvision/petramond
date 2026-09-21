//! The tool FAMILIES this pack animates — one data row per tool `kind` in
//! `families.json`, read once at init: the clips each combo step and the
//! work loop play on each rig, the pacing, and the strike profile. A row is
//! the whole of a family; adding a weapon family is adding a row.
//!
//! What the file cannot state is read off the rigs after parsing
//! ([`Families::resolve_impacts`]): where each attack clip marks its
//! `impact`, on the body rig (the instant the hit lands) and on the
//! first-person rig (the frame the wielder sees it land).

use crate::strike::Profile;
use mod_sdk::*;

pub const FAMILIES_JSON: &str = include_str!("../families.json");

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
    /// The phase of an attack's arc past which the NEXT attack may cut the
    /// recovery. The hold from the impact to here always plays whole:
    /// [`Family::cancel_at`] lifts it to the step's impact phase.
    pub cancel_at: f32,
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

    /// The phase step `combo`'s recovery opens to the next attack: the
    /// authored `cancel_at`, never before the step's impact.
    pub fn cancel_at(&self, combo: usize) -> f32 {
        self.pace
            .cancel_at
            .max(self.impact_phase(combo).unwrap_or(0.0))
    }
}

/// The table, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Families {
    rows: Vec<Family>,
}

impl Families {
    /// Parse the document. A row missing anything is refused whole — its
    /// kind then swings vanilla, never half a family — and answered beside
    /// the table for the caller to log.
    pub fn parse(text: &str) -> (Families, Vec<String>) {
        let Some(doc) = json::Value::parse(text).and_then(|v| v.as_object().map(<[_]>::to_vec))
        else {
            return (Families::default(), vec!["<document>".to_string()]);
        };
        let mut rows = Vec::new();
        let mut refused = Vec::new();
        for (kind, row) in &doc {
            match parse_family(kind, row) {
                Some(family) => rows.push(family),
                None => refused.push(kind.clone()),
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

fn parse_family(kind: &str, row: &json::Value) -> Option<Family> {
    let attacks: Vec<Motion> = row
        .get("attacks")?
        .as_array()?
        .iter()
        .map(motion)
        .collect::<Option<_>>()?;
    if attacks.is_empty() {
        return None;
    }
    let pace = row.get("pace")?;
    let attack: Vec<f32> = pace
        .get("window_attack")?
        .as_array()?
        .iter()
        .map(|v| v.as_f64().map(|n| n as f32))
        .collect::<Option<_>>()?;
    if attack.is_empty() || attack.iter().any(|w| *w <= 0.0) {
        return None;
    }
    let mine = num(pace, "window_mine")?;
    let cancel_at = num(pace, "cancel_at")?;
    if mine <= 0.0 || !(0.0..=1.0).contains(&cancel_at) {
        return None;
    }
    Some(Family {
        kind: kind.to_owned(),
        attacks,
        work: motion(row.get("work")?)?,
        pace: Pace {
            attack,
            mine,
            cancel_at,
        },
        profile: Profile::parse(row.get("profile")?)?,
        impacts: Vec::new(),
        fp_impacts: Vec::new(),
    })
}

fn motion(v: &json::Value) -> Option<Motion> {
    Some(Motion {
        first_person: v.get("fp")?.as_str()?.to_owned(),
        body: v.get("body")?.as_str()?.to_owned(),
    })
}

pub(crate) fn num(v: &json::Value, key: &str) -> Option<f32> {
    v.get(key)?.as_f64().map(|n| n as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = r#"{
        "attacks": [{"fp": "m:fp_a", "body": "m:body_a"}, {"fp": "m:fp_b", "body": "m:body_b"}],
        "work": {"fp": "m:fp_work", "body": "m:body_work"},
        "pace": {"window_attack": [0.5, 0.25], "window_mine": 0.4, "cancel_at": 0.3},
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

    /// A row is a family only WHOLE: the table keys `of_kind` in file
    /// order, a row missing a field or with an empty combo is refused
    /// alone, and the impacts are read off the rigs per step — the body's
    /// all or nothing, the viewmodel's positional.
    #[test]
    fn rows_parse_whole_and_impacts_come_off_the_rigs() {
        let doc = format!(
            r#"{{"hammer": {ROW}, "broken": {}, "bare": {}}}"#,
            ROW.replace(r#""work": {"fp": "m:fp_work", "body": "m:body_work"},"#, ""),
            ROW.replace(
                r#"[{"fp": "m:fp_a", "body": "m:body_a"}, {"fp": "m:fp_b", "body": "m:body_b"}]"#,
                "[]"
            ),
        );
        let (mut families, refused) = Families::parse(&doc);
        assert_eq!(
            families.styles().count(),
            1,
            "the broken rows are refused alone"
        );
        assert_eq!(refused, ["broken", "bare"]);
        let hammer = families
            .of_kind("hammer")
            .expect("the whole row is a family");
        assert_eq!(families.of_kind("broken"), None);
        assert_eq!(families.of_kind("bare"), None);
        assert_eq!(Families::parse("[]").0.styles().count(), 0);

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
        assert_eq!(
            family.cancel_at(0),
            0.5,
            "the hold begins at the impact, whatever the row says"
        );
        assert_eq!(
            family.cancel_at(1),
            0.3,
            "…and the row's cancel stands past it"
        );

        // One step without a body marker: the whole family lands nothing.
        let unlanded = families.resolve_impacts(|rig, clip| match (rig, clip) {
            (rig::PLAYER_BODY, "m:body_a") => Some(info(2.0, Some(1.0))),
            _ => None,
        });
        assert_eq!(unlanded, ["hammer"]);
        assert!(families.get(hammer).impacts.is_empty());
        assert_eq!(families.get(hammer).cancel_at(0), 0.3);
    }
}
