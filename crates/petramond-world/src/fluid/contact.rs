use crate::condition::{ConditionId, Pulse};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionGrant {
    pub condition: ConditionId,
    pub stage: u8,
    pub ticks: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FluidContact {
    pub damage: Option<Pulse>,
    pub applies: Option<ConditionGrant>,
    pub clears: &'static [ConditionId],
    pub destroys_items: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawContact {
    #[serde(default)]
    damage: Option<crate::condition::RawPulse>,
    #[serde(default)]
    applies: Option<RawGrant>,
    #[serde(default)]
    clears: Vec<String>,
    #[serde(default)]
    destroys_items: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RawGrant {
    condition: String,
    stage: String,
    ticks: u32,
}

impl RawContact {
    pub(crate) fn resolve(self) -> Result<FluidContact, String> {
        let condition = |key: &str| {
            crate::condition::by_name(key)
                .ok_or_else(|| format!("fluid contact names unknown condition '{key}'"))
        };
        let applies = self
            .applies
            .map(|g| -> Result<_, String> {
                let id = condition(&g.condition)?;
                let stage = id.def().stage(&g.stage).ok_or_else(|| {
                    format!("condition '{}' has no stage '{}'", g.condition, g.stage)
                })?;
                if g.ticks == 0 {
                    return Err("fluid contact applies.ticks must be positive".into());
                }
                Ok(ConditionGrant {
                    condition: id,
                    stage,
                    ticks: g.ticks,
                })
            })
            .transpose()?;
        let mut clears = self
            .clears
            .iter()
            .map(|k| condition(k))
            .collect::<Result<Vec<_>, _>>()?;
        clears.sort();
        clears.dedup();
        Ok(FluidContact {
            damage: self.damage.map(|d| d.resolve()).transpose()?,
            applies,
            clears: Box::leak(clears.into_boxed_slice()),
            destroys_items: self.destroys_items,
        })
    }
}
