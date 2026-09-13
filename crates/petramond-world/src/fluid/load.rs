use super::{
    contact::RawContact, CurrentProperties, FluidDef, FluidMedium, FluidMotion, FluidSplash, Quench,
};
use crate::{block::Block, registry::ContentNames};

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFluid {
    delay: u64,
    drop_off: u8,
    #[serde(default)]
    renewable: bool,
    #[serde(default)]
    quench: Option<RawQuench>,
    motion: FluidMotion,
    #[serde(default)]
    current: CurrentProperties,
    #[serde(default)]
    splash: Option<RawSplash>,
    #[serde(default)]
    contact: Option<RawContact>,
    medium: FluidMedium,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RawQuench {
    by: String,
    result: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RawSplash {
    burst: String,
    sound_small: String,
    sound_big: String,
}

impl RawSplash {
    fn resolve(self) -> Result<FluidSplash, String> {
        let burst = crate::particle_emitters::by_key(&self.burst)
            .filter(|bundle| bundle.burst.is_some())
            .ok_or_else(|| format!("fluid splash burst '{}' is not a burst bundle", self.burst))?;
        let sound = |key: &str| {
            crate::sound_registry::by_name(key)
                .ok_or_else(|| format!("fluid splash names unknown sound '{key}'"))
        };
        Ok(FluidSplash {
            burst: burst.id,
            sound_small: sound(&self.sound_small)?,
            sound_big: sound(&self.sound_big)?,
        })
    }
}

impl RawFluid {
    pub(crate) fn resolve(self, block: Block, names: &ContentNames) -> Result<FluidDef, String> {
        if self.delay == 0 || self.delay > 1200 || !(1..=7).contains(&self.drop_off) {
            return Err("fluid delay must be 1..=1200 ticks and drop_off 1..=7".into());
        }
        let m = self.motion;
        for (name, value, max) in [
            ("speed_scale", m.speed_scale, 4.0),
            ("accel", m.accel, 1000.0),
            ("friction", m.friction, 1.0),
            ("rise", m.rise, 30.0),
            ("sink", m.sink, 30.0),
            ("vertical_accel", m.vertical_accel, 1000.0),
            ("entry_friction", m.entry_friction, 1.0),
            ("probe_fraction", m.probe_fraction, 1.0),
            ("probe_offset", m.probe_offset, 1.0),
            ("current.speed", self.current.speed, 30.0),
            ("current.accel", self.current.accel, 1000.0),
        ] {
            if !value.is_finite() || !(0.0..=max).contains(&value) {
                return Err(format!("fluid {name} must be finite and in 0..={max}"));
            }
        }
        let resolve = |key: &str| {
            names
                .blocks
                .id(key)
                .map(Block)
                .ok_or_else(|| format!("unknown fluid reaction block '{key}'"))
        };
        let quench = self
            .quench
            .map(|q| -> Result<_, String> {
                Ok(Quench {
                    by: resolve(&q.by)?,
                    result: resolve(&q.result)?,
                })
            })
            .transpose()?;
        self.medium.validate()?;
        let contact = self
            .contact
            .map(RawContact::resolve)
            .transpose()?
            .unwrap_or_default();
        let name = names
            .blocks
            .name(block.id())
            .ok_or_else(|| "fluid block has no registered name".to_owned())?;
        Ok(FluidDef {
            block,
            name,
            delay: self.delay,
            drop_off: self.drop_off,
            renewable: self.renewable,
            quench,
            motion: m,
            current: self.current,
            splash: self.splash.map(RawSplash::resolve).transpose()?,
            contact,
            medium: self.medium,
        })
    }
}
