use serde::Deserialize;

use super::{ConditionDef, ConditionId, Pulse, StageDef, MAX_STAGES};
use crate::registry::Catalog;

#[derive(Deserialize)]
struct RawFile {
    conditions: Vec<RawCondition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCondition {
    condition: String,
    stages: Vec<RawStage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStage {
    stage: String,
    #[serde(default)]
    damage: Option<RawPulse>,
    #[serde(default)]
    emitter: Option<String>,
    #[serde(default)]
    cools_to: Option<String>,
    #[serde(default)]
    cools_after: Option<f32>,
}

#[derive(Clone, Copy, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPulse {
    amount: i32,
    interval: u32,
}

impl RawPulse {
    pub(crate) fn resolve(self) -> Result<Pulse, String> {
        if self.amount <= 0 || self.interval == 0 || self.interval > 72_000 {
            return Err("damage amount must be positive and interval 1..=72000 ticks".into());
        }
        Ok(Pulse {
            amount: self.amount,
            interval: self.interval,
        })
    }
}

pub(super) fn parse_layers(
    texts: &[&str],
    engine: &[&'static str],
    check_emitters: bool,
) -> Result<Catalog<ConditionDef>, String> {
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.conditions),
        |r| &r.condition,
        engine,
        "condition",
        |r, id, names| {
            let name = names.name(id).expect("id resolved from this table");
            let stages = resolve_stages(r.stages, check_emitters)
                .map_err(|e| format!("condition '{name}': {e}"))?;
            Ok(ConditionDef {
                id: ConditionId(id as u8),
                name,
                stages,
            })
        },
    )
}

fn resolve_stages(raw: Vec<RawStage>, check_emitters: bool) -> Result<&'static [StageDef], String> {
    if raw.is_empty() || raw.len() > MAX_STAGES {
        return Err(format!("declare 1..={MAX_STAGES} stages"));
    }
    let mut stages = Vec::with_capacity(raw.len());
    for (index, s) in raw.iter().enumerate() {
        if raw[..index].iter().any(|earlier| earlier.stage == s.stage) {
            return Err(format!("duplicate stage '{}'", s.stage));
        }
        let damage = s
            .damage
            .map(RawPulse::resolve)
            .transpose()
            .map_err(|e| format!("stage '{}': {e}", s.stage))?;
        if let Some(key) = &s.emitter {
            if check_emitters && crate::particle_emitters::by_key(key).is_none() {
                return Err(format!("stage '{}': unknown emitter '{key}'", s.stage));
            }
        }
        let (cools_to, cools_after) = match (&s.cools_to, s.cools_after) {
            (None, None) => (None, 1.0),
            (Some(to), Some(after)) => {
                // Only an EARLIER (weaker) stage keeps cooling chains finite.
                let target = raw[..index]
                    .iter()
                    .position(|earlier| &earlier.stage == to)
                    .ok_or_else(|| {
                        format!("stage '{}': cools_to must name an earlier stage", s.stage)
                    })?;
                if !after.is_finite() || after <= 0.0 || after > 1.0 {
                    return Err(format!(
                        "stage '{}': cools_after must be in (0, 1]",
                        s.stage
                    ));
                }
                (Some(target as u8), after)
            }
            _ => {
                return Err(format!(
                    "stage '{}': cools_to and cools_after go together",
                    s.stage
                ))
            }
        };
        stages.push(StageDef {
            name: Box::leak(s.stage.clone().into_boxed_str()),
            damage,
            emitter: s.emitter.clone().map(|k| &*Box::leak(k.into_boxed_str())),
            cools_to,
            cools_after,
        });
    }
    Ok(Box::leak(stages.into_boxed_slice()))
}
