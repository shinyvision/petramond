//! `ClientWorldMarksSet`: validate one of a mod's mark sets whole and retain
//! it beside the others.
//!
//! A set is accepted whole or refused whole. Drawing part of one would hand
//! the mod a picture it never asked for, and there is no render budget
//! downstream to cut it short either.

use mod_api::{ClientSprite, ClientWorldMark, HostRet};

use crate::modding::client::state::ClientStoreData;
use crate::modding::host::guards::{finite_pos, key_owned_by_namespace};

pub(super) fn set(
    client: &mut ClientStoreData,
    mod_id: &str,
    name: String,
    marks: Vec<ClientWorldMark>,
) -> HostRet {
    let valid_name = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !valid_name {
        return HostRet::invalid(format!(
            "ClientWorldMarksSet: invalid set name '{name}' (lowercase letters, digits, '_', \
             never empty)"
        ));
    }
    let mut kept = Vec::with_capacity(marks.len());
    for mark in marks {
        match accept(mark, mod_id) {
            Ok(mark) => kept.push(mark),
            Err(why) => return HostRet::invalid(format!("ClientWorldMarksSet: {why}")),
        }
    }
    if kept.is_empty() {
        client.world_marks.remove(&name);
    } else {
        client.world_marks.insert(name, kept);
    }
    HostRet::Unit
}

fn accept(mark: ClientWorldMark, mod_id: &str) -> Result<ClientWorldMark, String> {
    Ok(match mark {
        ClientWorldMark::Line {
            from,
            to,
            color,
            width,
            occluded,
        } => {
            if !(width.is_finite() && width > 0.0) {
                return Err(format!("line width {width} is not positive and finite"));
            }
            ClientWorldMark::Line {
                from: world_point(from)?,
                to: world_point(to)?,
                color,
                width,
                occluded,
            }
        }
        ClientWorldMark::Point {
            pos,
            sprite,
            size,
            color,
            label,
            occluded,
        } => {
            if !(size.is_finite() && size >= 0.0) {
                return Err(format!("point size {size} is negative or not finite"));
            }
            if let Some(ClientSprite::Image { key }) = &sprite {
                if !key_owned_by_namespace(mod_id, key) {
                    return Err(format!("image '{key}' must be namespaced '{mod_id}:name'"));
                }
            }
            if let Some(label) = &label {
                if label.contains(['\n', '\r']) {
                    return Err("a label is one line".into());
                }
            }
            ClientWorldMark::Point {
                pos: world_point(pos)?,
                sprite,
                size,
                color,
                label,
                occluded,
            }
        }
    })
}

/// A finite point inside the world border on every axis: the render origin a
/// mark is baked against is an `i32` cell, so y needs the bound x and z get.
fn world_point(v: [f64; 3]) -> Result<[f64; 3], String> {
    let border = f64::from(petramond_world::border::WORLD_BORDER);
    let [x, y, z] = finite_pos(v, "mark point")
        .map_err(|_| "a mark point has a non-finite component".to_string())?
        .to_array();
    Ok([x, y.clamp(-border, border), z])
}
