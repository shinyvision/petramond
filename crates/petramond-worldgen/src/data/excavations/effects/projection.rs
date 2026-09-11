use super::{block, Material, RawMaterial};
use crate::formula::{Expression, Formula};
use serde::{Deserialize, Serialize};

/// Project a field column's lowest matching cell onto neighbouring columns.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawProjection {
    pub from: Vec<String>,
    #[serde(default)]
    pub anchor: Option<RawAnchor>,
    pub offsets: Vec<[i32; 3]>,
    #[serde(default)]
    pub bindings: Vec<(String, Expression)>,
    pub when: Expression,
    pub range: [Expression; 2],
    #[serde(default)]
    pub probe: Option<RawProbe>,
    /// A negative result leaves the cell alone; other results select the palette.
    pub material: Expression,
    pub palette: Vec<RawMaterial>,
    #[serde(default)]
    pub avoid: Vec<String>,
    #[serde(default)]
    pub preserve: Vec<RawPreserve>,
    #[serde(default)]
    pub remap: Vec<[String; 2]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawAnchor {
    pub range: [Expression; 2],
    /// The matching predicate changes only from false to true over this range.
    #[serde(default)]
    pub monotone: bool,
}

pub struct Anchor {
    pub range: Formula,
    pub monotone: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawProbe {
    pub y: Expression,
    pub avoid: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawPreserve {
    pub from: Vec<String>,
    pub when: Expression,
}

pub struct Preserve {
    pub from: Vec<u16>,
    pub when: Formula,
}

pub struct Projection {
    pub from: Vec<u16>,
    pub anchor: Option<Anchor>,
    pub offsets: Vec<[i32; 3]>,
    pub admission: Formula,
    pub probe: Vec<u16>,
    pub probe_y: Formula,
    pub material: Formula,
    pub palette: Vec<Material>,
    pub avoid: Vec<u16>,
    pub preserve: Vec<Preserve>,
    pub remap: Vec<[u16; 2]>,
}

fn blocks(names: Vec<String>) -> Result<Vec<u16>, String> {
    names.iter().map(|name| block(name)).collect()
}

impl RawProjection {
    pub(in crate::data::excavations) fn resolve(
        self,
        parent: &[(String, Expression)],
    ) -> Result<Projection, String> {
        if self.from.is_empty()
            || self.from.len() > 16
            || self.offsets.is_empty()
            || self.offsets.len() > 6
            || self
                .offsets
                .iter()
                .any(|p| p.iter().map(|v| i64::from(*v).abs()).sum::<i64>() != 1)
            || self.palette.is_empty()
            || self.palette.len() > 16
            || self.preserve.len() > 8
            || self.remap.len() > 16
        {
            return Err(
                "invalid column projection: bounded palettes and cardinal offsets required".into(),
            );
        }
        let mut bindings = parent.to_vec();
        bindings.extend(self.bindings);
        let [lo, hi] = self.range;
        let (probe_y, probe) = self
            .probe
            .map_or((Expression::Number(0.0), Vec::new()), |p| (p.y, p.avoid));
        let probe_y = Formula::compile(&bindings, &[probe_y])?;
        if probe_y.uses_any(&[1]) {
            return Err("a projection probe needs a height independent of its anchor".into());
        }
        let anchor = self
            .anchor
            .map(|anchor| -> Result<Anchor, String> {
                let range = Formula::compile(&bindings, &anchor.range)?;
                if range.uses_any(&[1]) {
                    return Err("an anchor range must be independent of scan height".into());
                }
                Ok(Anchor {
                    range,
                    monotone: anchor.monotone,
                })
            })
            .transpose()?;
        Ok(Projection {
            from: blocks(self.from)?,
            anchor,
            offsets: self.offsets,
            admission: Formula::compile(&bindings, &[self.when, lo, hi])?,
            probe: blocks(probe)?,
            probe_y,
            material: Formula::compile(&bindings, &[self.material])?,
            palette: self
                .palette
                .into_iter()
                .map(RawMaterial::resolve)
                .collect::<Result<_, _>>()?,
            avoid: blocks(self.avoid)?,
            preserve: self
                .preserve
                .into_iter()
                .map(|p| {
                    Ok(Preserve {
                        from: blocks(p.from)?,
                        when: Formula::compile(&bindings, &[p.when])?,
                    })
                })
                .collect::<Result<_, String>>()?,
            remap: self
                .remap
                .into_iter()
                .map(|[a, b]| Ok([block(&a)?, block(&b)?]))
                .collect::<Result<_, String>>()?,
        })
    }
}
