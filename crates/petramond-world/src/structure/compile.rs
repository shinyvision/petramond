use std::collections::{BTreeMap, BTreeSet};

use crate::block::Block;
use crate::mathh::IVec3;
use crate::world::placement::authored::{self, Expansion, Inputs, Turn};

use super::{schema, Bounds, Connector, Template, Variant};

const MAX_VOLUME: usize = 131_072;
const MAX_OPERATIONS: usize = 262_144;

pub(super) fn compile(
    raw: schema::Template,
    resolve: impl Fn(&str) -> Option<Block>,
) -> Result<Template, String> {
    let size = IVec3::from(raw.size.map(i32::from));
    if raw.size.iter().any(|&n| n == 0 || n > 128)
        || raw.size.iter().map(|&n| n as usize).product::<usize>() > MAX_VOLUME
    {
        return Err("template size exceeds 128 per axis or 131072 cells".into());
    }
    let bounds = Bounds {
        min: IVec3::ZERO,
        max: size - IVec3::ONE,
    };
    let pivot = IVec3::from(raw.pivot);
    if !bounds.contains(pivot) {
        return Err("pivot outside template bounds".into());
    }
    let mut probe_count = 0;
    if raw.requirements.len() > 64 {
        return Err("template exceeds terrain requirement budget".into());
    }
    let mut requirements = raw.requirements;
    for region in &mut requirements {
        probe_count += region
            .cell_count()
            .ok_or("terrain requirement is reversed or oversized")?;
        let min = IVec3::from(region.min);
        let max = IVec3::from(region.max);
        if probe_count > mod_api::STRUCTURE_PROBES_MAX
            || !min.cmpge(bounds.min - IVec3::ONE).all()
            || !max.cmple(bounds.max + IVec3::ONE).all()
        {
            return Err(
                "terrain requirements exceed probe budget or template's one-cell margin".into(),
            );
        }
        region.min = (min - pivot).to_array();
        region.max = (max - pivot).to_array();
    }
    if raw.palette.len() > 256 || raw.markers.len() > 256 || raw.connectors.len() > 64 {
        return Err("template exceeds palette, marker or connector budget".into());
    }
    let palette = raw
        .palette
        .iter()
        .map(|(key, material)| {
            let block = resolve(&material.block)
                .ok_or_else(|| format!("palette '{key}': unknown block '{}'", material.block))?;
            Ok((key.as_str(), (block, &material.state)))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let mut entries = Vec::new();
    for fill in &raw.fills {
        let from = IVec3::from(fill.from);
        let to = IVec3::from(fill.to);
        if !bounds.contains(from) || !bounds.contains(to) || !from.cmple(to).all() {
            return Err(format!(
                "fill '{}' outside bounds or reversed",
                fill.palette
            ));
        }
        let volume = (to - from + IVec3::ONE)
            .to_array()
            .into_iter()
            .map(|n| n as usize)
            .product::<usize>();
        if entries.len() + volume > MAX_OPERATIONS {
            return Err("template expansion exceeds operation budget".into());
        }
        for y in from.y..=to.y {
            for z in from.z..=to.z {
                for x in from.x..=to.x {
                    entries.push((IVec3::new(x, y, z), fill.palette.as_str()));
                }
            }
        }
    }
    if entries.len() + raw.blocks.len() > MAX_OPERATIONS {
        return Err("template expansion exceeds operation budget".into());
    }
    for block in &raw.blocks {
        let pos = IVec3::from(block.pos);
        if !bounds.contains(pos) {
            return Err(format!("block at {pos:?} outside bounds"));
        }
        entries.push((pos, block.palette.as_str()));
    }
    let mut connector_names = BTreeSet::new();
    for connector in &raw.connectors {
        if connector.name.is_empty()
            || !connector_names.insert(&connector.name)
            || !crate::registry::is_namespaced(&connector.kind)
            || !bounds.contains(IVec3::from(connector.pos))
        {
            return Err(format!(
                "invalid or duplicate connector '{}'",
                connector.name
            ));
        }
    }
    let variants = Turn::ALL
        .into_iter()
        .map(|turn| {
            let a = turn.apply(bounds.min - pivot);
            let b = turn.apply(bounds.max - pivot);
            let rotated_bounds = Bounds {
                min: a.min(b),
                max: a.max(b),
            };
            let layouts = palette
                .iter()
                .map(|(&key, (block, state))| {
                    authored::layout(*block, state, turn)
                        .map(|writes| (key, writes))
                        .map_err(|e| format!("palette '{key}': {e}"))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            let mut expansion = Expansion::default();
            for &(pos, key) in &entries {
                let writes = layouts
                    .get(key)
                    .ok_or_else(|| format!("unknown palette entry '{key}'"))?;
                if expansion.writes() + writes.len() > MAX_OPERATIONS {
                    return Err("expanded object footprints exceed operation budget".into());
                }
                let anchor = turn.apply(pos - pivot);
                if writes
                    .iter()
                    .any(|write| !rotated_bounds.contains(anchor + write.cell))
                {
                    return Err(format!(
                        "'{key}' footprint extends outside template at {pos:?}"
                    ));
                }
                expansion.place(anchor, writes);
            }
            let mut cells = expansion.finish()?;
            for marker in &raw.markers {
                if !crate::registry::is_namespaced(&marker.key)
                    || marker.key.len() > mod_api::KV_MAX_KEY_BYTES
                    || !bounds.contains(IVec3::from(marker.pos))
                {
                    return Err(format!("invalid marker '{}'", marker.key));
                }
                let pos = turn.apply(IVec3::from(marker.pos) - pivot);
                let Some(cell) = cells.get_mut(pos.to_array()) else {
                    return Err(format!(
                        "marker '{}' needs an authored cell at {:?}",
                        marker.key, marker.pos
                    ));
                };
                let value = serde_json::to_vec(&marker.value).map_err(|e| e.to_string())?;
                if value.len() > mod_api::KV_MAX_VALUE_BYTES
                    || cell.data.len() >= mod_api::CELL_KV_MAX_KEYS
                    || cell.data.insert(marker.key.clone(), value).is_some()
                {
                    return Err(format!("duplicate or oversized marker '{}'", marker.key));
                }
            }
            let connectors = raw
                .connectors
                .iter()
                .map(|c| {
                    let props = BTreeMap::from([("facing".into(), c.facing.clone())]);
                    let mut inputs = Inputs::new(IVec3::ZERO, turn, &props);
                    Ok(Connector {
                        name: c.name.clone(),
                        kind: c.kind.clone(),
                        pos: turn.apply(IVec3::from(c.pos) - pivot),
                        facing: inputs.facing()?,
                    })
                })
                .collect::<Result<_, String>>()?;
            Ok(Variant {
                bounds: rotated_bounds,
                cells: cells.into_cells(),
                connectors,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Template {
        variants: variants.try_into().ok().unwrap(),
        requirements,
    })
}
