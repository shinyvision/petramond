//! The client-instance host-call handlers: the whole [`ClientCall`] domain,
//! size/namespace-capped, plus the read-only replica scope, and the
//! [`BodyCall`] domain answered as PREDICTIONS against the local mirror.
//! Which calls a client instance may make at all is the switchboard's
//! decision, read from each call's declared legality.

#[cfg(test)]
mod tests;
mod validate;

use std::sync::Arc;

use mod_api::{BodyCall, ClientCall, ErrorCode, HostRet};
use petramond_world::item::ItemType;

use crate::modding::host::guards::{key_owned_by_namespace, KV_MAX_KEY_BYTES};
use crate::modding::host::ModStoreData;

use super::scope as client_scope;
use super::state::{ClientCommand, ClientImageData, ClientOverlayRegistration};
use validate::{
    client_canvas_element_image_key, client_canvas_element_valid, valid_client_key,
    valid_client_key_id, CLIENT_AMBIENT_WIND_MAX, CLIENT_BLOCKS_QUERY_MAX,
    CLIENT_CANVAS_ELEMENT_MAX, CLIENT_CANVAS_MAX, CLIENT_CANVAS_SIDE_MAX, CLIENT_COMMAND_MAX,
    CLIENT_ENV_PARAM_MAX, CLIENT_IMAGE_MAX, CLIENT_IMAGE_SIDE_MAX, CLIENT_KEY_BINDING_MAX,
    CLIENT_OVERLAY_DISPLAY_SIDE_MAX, CLIENT_OVERLAY_MAX, CLIENT_SURFACE_QUERY_MAX,
    CLIENT_TEXT_BYTES_MAX, CLIENT_TEXT_RUN_MAX, CLIENT_TEXT_SCALE_MAX, CLIENT_UI_STATE_MAX,
};

/// The presentation surface. Only a client instance reaches it (the
/// switchboard admits a call by its declared sides), and the registrations
/// only inside `mod_init` (their declared scope).
pub(in crate::modding) fn handle_client_call(data: &mut ModStoreData, call: ClientCall) -> HostRet {
    let mod_id = data.mod_id.clone();
    let Some(client) = data.client.as_mut() else {
        return HostRet::invalid("client instance has no client state".into());
    };
    match call {
        ClientCall::ClientRegisterOverlay {
            image_key,
            anchor,
            margin,
            display_size,
        } => {
            if !key_owned_by_namespace(&mod_id, &image_key) {
                return HostRet::invalid(format!(
                    "client overlay image '{image_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if display_size[0] == 0
                || display_size[1] == 0
                || display_size[0] > CLIENT_OVERLAY_DISPLAY_SIDE_MAX
                || display_size[1] > CLIENT_OVERLAY_DISPLAY_SIDE_MAX
            {
                return HostRet::invalid(format!(
                    "invalid client overlay display size {}x{}",
                    display_size[0], display_size[1]
                ));
            }
            if client.overlays.len() >= CLIENT_OVERLAY_MAX
                && !client
                    .overlays
                    .iter()
                    .any(|overlay| overlay.image_key == image_key)
            {
                return HostRet::invalid("client overlay registration limit reached".into());
            }
            if !client
                .overlays
                .iter()
                .any(|overlay| overlay.image_key == image_key)
            {
                client.overlays.push(ClientOverlayRegistration {
                    image_key,
                    anchor,
                    margin,
                    display_size,
                });
            }
            HostRet::Unit
        }
        ClientCall::ClientRegisterKey {
            id,
            label,
            key,
            action_id,
        } => {
            if !valid_client_key_id(&id) {
                return HostRet::invalid(format!(
                    "invalid client key id '{id}' (bare lowercase snake_case, max 48 chars)"
                ));
            }
            if label.trim().is_empty() || label.len() > 48 {
                return HostRet::invalid(format!("invalid client key label '{label}'"));
            }
            if !valid_client_key(&key) {
                return HostRet::invalid(format!("unsupported client key '{key}'"));
            }
            if client.key_bindings.iter().any(|b| b.id == id) {
                return HostRet::invalid(format!("client key id '{id}' registered twice"));
            }
            if client.key_bindings.len() >= CLIENT_KEY_BINDING_MAX {
                return HostRet::invalid("client key registration limit reached".into());
            }
            client.key_bindings.push(super::state::ClientKeyBinding {
                id,
                label,
                key,
                action_id,
            });
            HostRet::Unit
        }
        ClientCall::ClientEnvParams { keys } => {
            if keys.len() > CLIENT_ENV_PARAM_MAX {
                return HostRet::invalid(format!(
                    "ClientEnvParams key count {} exceeds {CLIENT_ENV_PARAM_MAX}",
                    keys.len()
                ));
            }
            client_scope::with_active(|world| {
                let params = world.data().environment().shader_params().clone();
                HostRet::EnvParams(keys.iter().map(|k| params.get(k).copied()).collect())
            })
            .unwrap_or_else(|| HostRet::invalid("no client replica is active".into()))
        }
        ClientCall::ClientBiomeAt { pos } => client_scope::with_active(|world| {
            HostRet::MaybeByte(world.data().biome_at_world(pos[0], pos[1]))
        })
        .unwrap_or_else(|| HostRet::invalid("no client replica is active".into())),
        ClientCall::ClientBlocksAt { positions } => {
            if positions.len() > CLIENT_BLOCKS_QUERY_MAX {
                return HostRet::invalid(format!(
                    "ClientBlocksAt position count {} exceeds {CLIENT_BLOCKS_QUERY_MAX}",
                    positions.len()
                ));
            }
            client_scope::with_active(|world| {
                HostRet::Blocks(
                    positions
                        .iter()
                        .map(|&[x, y, z]| {
                            world
                                .block_if_stream_final(x, y, z)
                                .map(|b| mod_api::BlockId(b.id()))
                        })
                        .collect(),
                )
            })
            .unwrap_or_else(|| HostRet::invalid("no client replica is active".into()))
        }
        ClientCall::ClientCellKvAt { key, cells } => {
            if cells.len() > CLIENT_BLOCKS_QUERY_MAX {
                return HostRet::invalid(format!(
                    "ClientCellKvAt cell count {} exceeds {CLIENT_BLOCKS_QUERY_MAX}",
                    cells.len()
                ));
            }
            // Reads cross namespaces (the KV interop contract); only the key's
            // shape is validated, like the server-side KV read.
            if key.is_empty() || key.len() > KV_MAX_KEY_BYTES {
                return HostRet::invalid(format!("invalid cell KV key '{key}'"));
            }
            client_scope::with_active(|world| {
                HostRet::BytesMany(
                    cells
                        .iter()
                        .map(|&[x, y, z]| world.data().cell_kv_get(x, y, z, &key).map(<[u8]>::to_vec))
                        .collect(),
                )
            })
            .unwrap_or_else(|| HostRet::invalid("no client replica is active".into()))
        }
        ClientCall::ClientAmbientSet {
            key,
            intensity,
            wind,
        } => {
            if !intensity.is_finite()
                || !wind
                    .iter()
                    .all(|w| w.is_finite() && w.abs() <= CLIENT_AMBIENT_WIND_MAX)
            {
                return HostRet::invalid(
                    "ClientAmbientSet: intensity and wind must be finite (|wind| ≤ 64)".into(),
                );
            }
            // Unknown keys and non-ambient bundles are forgiving `false`
            // (a disabled pack's bundle is not a protocol break).
            let Some(bundle) = petramond_world::particle_emitters::by_key(&key) else {
                return HostRet::Bool(false);
            };
            if bundle.ambient.is_none() {
                return HostRet::Bool(false);
            }
            client
                .ambient_sets
                .insert(bundle.id, (intensity.clamp(0.0, 1.0), wind));
            HostRet::Bool(true)
        }
        ClientCall::ClientLoopSet { key, gain } => {
            if !gain.is_finite() {
                return HostRet::invalid("ClientLoopSet: gain must be finite".into());
            }
            let Some(sound) = petramond_world::sound_registry::by_name(&key) else {
                return HostRet::Bool(false);
            };
            client.sound_loops.insert(sound, gain.clamp(0.0, 4.0));
            HostRet::Bool(true)
        }
        ClientCall::ClientMoodSet { darken, desaturate } => {
            if !darken.is_finite() || !desaturate.is_finite() {
                return HostRet::invalid("ClientMoodSet: values must be finite".into());
            }
            // The clamp IS the safety contract: no mod can black the screen
            // out; it can only be moody about it.
            client.mood = [darken.clamp(0.0, 0.5), desaturate.clamp(0.0, 0.5)];
            HostRet::Bool(true)
        }
        ClientCall::ClientSurfaceColumns { queries } => {
            if queries.len() > CLIENT_SURFACE_QUERY_MAX {
                return HostRet::invalid(format!(
                    "ClientSurfaceColumns query count {} exceeds {CLIENT_SURFACE_QUERY_MAX}",
                    queries.len()
                ));
            }
            client_scope::with_active(|world| {
                let mut cells = [None::<(i16, [u8; 3])>; 256];
                let columns = queries
                    .iter()
                    .map(|query| {
                        let pos =
                            petramond_world::chunk::ChunkPos::new(query.coord[0], query.coord[1]);
                        let revision = world.client_surface_column_revision(pos)?;
                        // A zero query revision means "never seen complete" —
                        // it must never match, even against a defaulted host
                        // revision.
                        if query.revision != 0 && query.revision == revision {
                            return Some(mod_api::ClientSurfaceColumn {
                                revision,
                                cells: None,
                            });
                        }
                        if !world.client_surface_column(pos, &mut cells) {
                            return None;
                        }
                        let mut packed = Vec::with_capacity(mod_api::CLIENT_SURFACE_COLUMN_BYTES);
                        for cell in &cells {
                            let (height, rgb) =
                                cell.unwrap_or((mod_api::CLIENT_SURFACE_UNKNOWN_HEIGHT, [0; 3]));
                            packed.extend_from_slice(&height.to_le_bytes());
                            packed.extend_from_slice(&rgb);
                        }
                        Some(mod_api::ClientSurfaceColumn {
                            revision,
                            cells: Some(packed),
                        })
                    })
                    .collect();
                HostRet::ClientSurfaceColumns(columns)
            })
            .unwrap_or_else(|| HostRet::invalid("no client replica is active".into()))
        }
        ClientCall::ClientUiStateSet { key, value } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::invalid(format!(
                    "client UI key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if !validate::gui_value_fits(&value) {
                return HostRet::invalid("client UI value exceeds its size limit".into());
            }
            if !client.ui_state.contains_key(&key) && client.ui_state.len() >= CLIENT_UI_STATE_MAX {
                return HostRet::invalid("client UI state entry limit reached".into());
            }
            Arc::make_mut(&mut client.ui_state).insert(key, value);
            HostRet::Unit
        }
        ClientCall::ClientUiStateGet { key } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::invalid(format!(
                    "client UI key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            HostRet::GuiValue(client.ui_state.get(&key).cloned())
        }
        ClientCall::ClientImageSet {
            key,
            width,
            height,
            rgba,
        } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::invalid(format!(
                    "client image key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if width == 0
                || height == 0
                || width > CLIENT_IMAGE_SIDE_MAX
                || height > CLIENT_IMAGE_SIDE_MAX
                || rgba.len() != width as usize * height as usize * 4
            {
                return HostRet::invalid(format!(
                    "invalid client image {width}x{height} with {} RGBA bytes",
                    rgba.len()
                ));
            }
            if !client.images.contains_key(&key) && client.images.len() >= CLIENT_IMAGE_MAX {
                return HostRet::invalid("client image limit reached".into());
            }
            let revision = client.next_image_revision;
            client.next_image_revision = client.next_image_revision.wrapping_add(1).max(1);
            client.images.insert(
                key.clone(),
                ClientImageData {
                    key,
                    width,
                    height,
                    rgba: Arc::from(rgba.into_boxed_slice()),
                    revision,
                    recent_blits: Vec::new(),
                },
            );
            HostRet::Unit
        }
        ClientCall::ClientImageBlit {
            key,
            origin,
            size,
            rgba,
        } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::invalid(format!(
                    "client image key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            let Some((width, height)) = client
                .images
                .get(&key)
                .map(|image| (image.width as usize, image.height as usize))
            else {
                return HostRet::invalid(format!("client image '{key}' has not been published"));
            };
            let (w, h) = (size[0] as usize, size[1] as usize);
            if w == 0
                || h == 0
                || origin[0] as usize + w > width
                || origin[1] as usize + h > height
                || rgba.len() != w * h * 4
            {
                return HostRet::invalid(format!(
                    "invalid client image blit {w}x{h} at ({}, {}) with {} RGBA bytes into {width}x{height}",
                    origin[0],
                    origin[1],
                    rgba.len()
                ));
            }
            let revision = client.next_image_revision;
            client.next_image_revision = client.next_image_revision.wrapping_add(1).max(1);
            let image = client.images.get_mut(&key).unwrap();
            let dst = Arc::make_mut(&mut image.rgba);
            for row in 0..h {
                let src = row * w * 4;
                let at = ((origin[1] as usize + row) * width + origin[0] as usize) * 4;
                dst[at..at + w * 4].copy_from_slice(&rgba[src..src + w * 4]);
            }
            image.revision = revision;
            if image.recent_blits.len() >= super::state::IMAGE_BLIT_WINDOW {
                image.recent_blits.remove(0);
            }
            image
                .recent_blits
                .push((revision, [origin[0], origin[1], size[0], size[1]]));
            HostRet::Unit
        }
        ClientCall::ClientTextMeasure { text, scale } => {
            if scale == 0 || scale > CLIENT_TEXT_SCALE_MAX {
                return HostRet::invalid(format!(
                    "client text scale {scale} must be 1..={CLIENT_TEXT_SCALE_MAX}"
                ));
            }
            if text.len() > CLIENT_TEXT_BYTES_MAX || text.contains(['\n', '\r']) {
                return HostRet::invalid("invalid single-line client text".into());
            }
            // Mod canvases measure and draw with the UI theme's font.
            let [width, height] = crate::gui::doc_theme::ui_font().measure_scaled(&text, scale);
            let Ok(width) = u16::try_from(width) else {
                return HostRet::invalid("client text width exceeds u16".into());
            };
            let Ok(height) = u16::try_from(height) else {
                return HostRet::invalid("client text height exceeds u16".into());
            };
            HostRet::ClientTextSize([width, height])
        }
        ClientCall::ClientImageDrawTexts { key, runs } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::invalid(format!(
                    "client image key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if runs.len() > CLIENT_TEXT_RUN_MAX
                || runs.iter().map(|run| run.text.len()).sum::<usize>() > CLIENT_TEXT_BYTES_MAX
                || runs.iter().any(|run| {
                    run.scale == 0
                        || run.scale > CLIENT_TEXT_SCALE_MAX
                        || run.text.contains(['\n', '\r'])
                })
            {
                return HostRet::invalid("invalid client text run batch".into());
            }
            if !client.images.contains_key(&key) {
                return HostRet::invalid(format!("client image '{key}' has not been published"));
            }
            let revision = client.next_image_revision;
            client.next_image_revision = client.next_image_revision.wrapping_add(1).max(1);
            let image = client.images.get_mut(&key).unwrap();
            let rgba = Arc::make_mut(&mut image.rgba);
            let font = crate::gui::doc_theme::ui_font();
            for run in runs {
                font.draw_rgba(
                    rgba,
                    image.width as u32,
                    &run.text,
                    run.position,
                    run.scale,
                    run.color,
                );
            }
            image.revision = revision;
            // Text bounds aren't tracked as a rect: break the partial-update
            // chain so consumers re-upload the whole image once.
            image.recent_blits.clear();
            HostRet::Unit
        }
        ClientCall::ClientGuiOpen { kind_key } => {
            if !key_owned_by_namespace(&mod_id, &kind_key) {
                return HostRet::invalid(format!(
                    "client GUI kind '{kind_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::invalid("client GUI command queue limit reached".into());
            }
            client.commands.push(ClientCommand::OpenGui {
                owner: mod_id,
                kind: kind_key,
            });
            HostRet::Bool(true)
        }
        ClientCall::ClientGuiClose => {
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::invalid("client GUI command queue limit reached".into());
            }
            client
                .commands
                .push(ClientCommand::CloseGui { owner: mod_id });
            HostRet::Unit
        }
        ClientCall::ClientCanvasOpen { canvas_key, size } => {
            if !key_owned_by_namespace(&mod_id, &canvas_key) {
                return HostRet::invalid(format!(
                    "client canvas '{canvas_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if size[0] == 0
                || size[1] == 0
                || size[0] > CLIENT_CANVAS_SIDE_MAX
                || size[1] > CLIENT_CANVAS_SIDE_MAX
            {
                return HostRet::invalid(format!(
                    "invalid client canvas size {}x{}",
                    size[0], size[1]
                ));
            }
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::invalid("client command queue limit reached".into());
            }
            client.commands.push(ClientCommand::OpenCanvas {
                owner: mod_id,
                canvas_key,
                size,
            });
            HostRet::Bool(true)
        }
        ClientCall::ClientCanvasClose => {
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::invalid("client command queue limit reached".into());
            }
            client
                .commands
                .push(ClientCommand::CloseCanvas { owner: mod_id });
            HostRet::Unit
        }
        ClientCall::ClientCanvasSceneSet {
            canvas_key,
            elements,
        } => {
            if !key_owned_by_namespace(&mod_id, &canvas_key) {
                return HostRet::invalid(format!(
                    "client canvas key '{canvas_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if elements.len() > CLIENT_CANVAS_ELEMENT_MAX {
                return HostRet::invalid("client canvas element limit reached".into());
            }
            if let Some(image_key) = elements
                .iter()
                .map(client_canvas_element_image_key)
                .find(|key| !key_owned_by_namespace(&mod_id, key))
            {
                return HostRet::invalid(format!(
                    "client canvas image '{}' must be namespaced '{mod_id}:name'",
                    image_key
                ));
            }
            if elements
                .iter()
                .any(|element| !client_canvas_element_valid(element))
            {
                return HostRet::invalid("client canvas element geometry is invalid".into());
            }
            if !client.canvas_scenes.contains_key(&canvas_key)
                && client.canvas_scenes.len() >= CLIENT_CANVAS_MAX
            {
                return HostRet::invalid("client canvas limit reached".into());
            }
            client.canvas_scenes.entry(canvas_key).or_default().elements = elements;
            HostRet::Unit
        }
        ClientCall::ClientCanvasViewSet { canvas_key, offset } => {
            if !key_owned_by_namespace(&mod_id, &canvas_key) {
                return HostRet::invalid(format!(
                    "client canvas key '{canvas_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if !offset[0].is_finite() || !offset[1].is_finite() {
                return HostRet::invalid("client canvas view offset must be finite".into());
            }
            if !client.canvas_scenes.contains_key(&canvas_key)
                && client.canvas_scenes.len() >= CLIENT_CANVAS_MAX
            {
                return HostRet::invalid("client canvas limit reached".into());
            }
            client.canvas_scenes.entry(canvas_key).or_default().offset = offset;
            HostRet::Unit
        }
        ClientCall::ClientStorageGetMany { keys } => {
            for key in &keys {
                if !key_owned_by_namespace(&mod_id, key) {
                    return HostRet::invalid(format!(
                        "client storage key '{key}' must be namespaced '{mod_id}:name'"
                    ));
                }
            }
            match client.storage.get_many(&keys) {
                Ok(values) => HostRet::ClientStorageValues(
                    values
                        .into_iter()
                        .map(|value| value.map(mod_api::ByteBuf::from))
                        .collect(),
                ),
                Err(error) => HostRet::invalid(error),
            }
        }
        ClientCall::ClientStorageReadBegin { keys } => {
            for key in &keys {
                if !key_owned_by_namespace(&mod_id, key) {
                    return HostRet::invalid(format!(
                        "client storage key '{key}' must be namespaced '{mod_id}:name'"
                    ));
                }
            }
            match client.storage.read_begin(keys) {
                Ok(ticket) => HostRet::U64(ticket),
                Err(error) => HostRet::invalid(error),
            }
        }
        ClientCall::ClientStorageReadPoll { ticket } => match client.storage.read_poll(ticket) {
            Ok(values) => HostRet::ClientStorageRead(values.map(|values| {
                values
                    .into_iter()
                    .map(|value| value.map(mod_api::ByteBuf::from))
                    .collect()
            })),
            Err(error) => HostRet::invalid(error),
        },
        ClientCall::ClientStorageSetMany { entries } => {
            for (key, value) in &entries {
                if !key_owned_by_namespace(&mod_id, key) {
                    return HostRet::invalid(format!(
                        "client storage key '{key}' must be namespaced '{mod_id}:name'"
                    ));
                }
                if key.len() > super::storage::KEY_MAX || value.len() > super::storage::VALUE_MAX {
                    return HostRet::invalid(format!(
                        "client storage entry '{key}' exceeds key/value limits"
                    ));
                }
            }
            let entries = entries
                .into_iter()
                .map(|(key, value)| (key, value.into_vec()))
                .collect();
            match client.storage.set_many(entries) {
                Ok(()) => HostRet::Bool(true),
                Err(error) => HostRet::invalid(error),
            }
        }
    }
}

/// The body domain on a client instance: every write is a PREDICTION
/// against the local mirror, addressed at the local player, and the actor is
/// the snapshot the prediction dispatch published (see `scope::enter_actor`)
/// — the same query-the-snapshot doctrine as the server side.
pub(in crate::modding) fn handle_body_call(data: &mut ModStoreData, call: BodyCall) -> HostRet {
    let mod_id = data.mod_id.clone();
    let Some(client) = data.client.as_mut() else {
        return HostRet::invalid("client instance has no client state".into());
    };
    match call {
        BodyCall::PlayerState => match super::scope::active_actor() {
            Some(actor) => HostRet::Player(Box::new(actor)),
            None => HostRet::error(
                ErrorCode::NoContext,
                "PlayerState on a client instance is available during prediction dispatches only"
                    .into(),
            ),
        },
        // Outside a prediction dispatch a client instance acts for nobody.
        BodyCall::ActingPlayer => {
            HostRet::ActingPlayer(super::scope::active_actor().and_then(|actor| actor.id))
        }
        // The PREDICTED twin of the server's `SetPlayerHeldPose`: the same
        // call, the same `BodyClaims`, addressed at the local player. A client
        // has exactly one addressable body, so naming anybody else is a mod
        // bug worth saying out loud rather than a silent no-op.
        BodyCall::SetPlayerHeldPose { player, main, off } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "SetPlayerHeldPose: a client instance may pose only the LOCAL player \
                     ({local:?}), not {player:?}"
                ));
            }
            client.poses_hands[0] |= main.is_some();
            client.poses_hands[1] |= off.is_some();
            if client.body.set_held_pose(&mod_id, main, off) {
                HostRet::Bool(true)
            } else {
                HostRet::invalid(
                    "SetPlayerHeldPose: non-finite rotation/translation component".into(),
                )
            }
        }
        // The PREDICTED twins of the server's animator primitives, addressed
        // at the local player like every body write here. The latch is per
        // `(rig, param)` / `(rig, slot)`, like bones: a mod setting one param
        // owns that param locally from then on, and leaves every other
        // replicated claim exactly where it was. A refused write latches
        // nothing, or a NaN would hide the replicated claim for good.
        BodyCall::SetPlayerAnimatorParams { player, params } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "SetPlayerAnimatorParams: a client instance may animate only the LOCAL player \
                     ({local:?}), not {player:?}"
                ));
            }
            let params = match crate::player::animator::resolve_params(params) {
                Ok(params) => params,
                Err(e) => return HostRet::invalid(format!("SetPlayerAnimatorParams: {e}")),
            };
            let keys: Vec<_> = params.iter().map(|p| (p.rig, p.param)).collect();
            if !client.body.set_animator_params(&mod_id, params) {
                return HostRet::invalid("SetPlayerAnimatorParams: non-finite value".into());
            }
            client.owns_animator.params.extend(keys);
            HostRet::Bool(true)
        }
        BodyCall::SetPlayerAnimatorPlays { player, plays } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "SetPlayerAnimatorPlays: a client instance may animate only the LOCAL player \
                     ({local:?}), not {player:?}"
                ));
            }
            let plays = match crate::player::animator::resolve_plays(plays) {
                Ok(plays) => plays,
                Err(e) => return HostRet::invalid(format!("SetPlayerAnimatorPlays: {e}")),
            };
            let keys: Vec<_> = plays.iter().map(|p| (p.rig, p.slot)).collect();
            if !client.body.set_animator_plays(&mod_id, plays) {
                return HostRet::invalid(
                    "SetPlayerAnimatorPlays: non-finite progress or rate".into(),
                );
            }
            client.owns_animator.slots.extend(keys);
            HostRet::Bool(true)
        }
        BodyCall::FirePlayerAnimatorEvent { player, rig, event } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "FirePlayerAnimatorEvent: a client instance may animate only the LOCAL player \
                     ({local:?}), not {player:?}"
                ));
            }
            let (rig, event) = match crate::player::animator::resolve_event(&rig, &event) {
                Ok(resolved) => resolved,
                Err(e) => return HostRet::invalid(format!("FirePlayerAnimatorEvent: {e}")),
            };
            client.animator_events.push((rig, event));
            HostRet::Bool(true)
        }
        BodyCall::AnimationClip { rig, clip } => {
            HostRet::AnimationClip(crate::player::animator::clip_info(&rig, &clip))
        }
        BodyCall::PlayerInventory { player } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "PlayerInventory: a client instance may read only the LOCAL player \
                     ({local:?}), not {player:?}"
                ));
            }
            match super::scope::with_inventory(super::super::host::player::carried_slots) {
                Some(slots) => HostRet::ContainerSlots(Some(slots)),
                None => HostRet::invalid("PlayerInventory: no inventory is published".into()),
            }
        }
        // What the hand displays: the same predicted path, the same latch.
        BodyCall::SetPlayerHeldDisplay { player, main, off } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "SetPlayerHeldDisplay: a client instance may dress only the LOCAL player \
                     ({local:?}), not {player:?}"
                ));
            }
            let item = |name: &Option<String>| match name {
                None => Ok(None),
                Some(name) => ItemType::by_name(name).map(Some).ok_or_else(|| {
                    HostRet::invalid(format!("SetPlayerHeldDisplay: unknown item '{name}'"))
                }),
            };
            let (main, off) = match (item(&main), item(&off)) {
                (Ok(main), Ok(off)) => (main, off),
                (Err(e), _) | (_, Err(e)) => return e,
            };
            client.displays_hands[0] |= main.is_some();
            client.displays_hands[1] |= off.is_some();
            client.body.set_held_display(&mod_id, main, off);
            HostRet::Bool(true)
        }
        // The body counterpart, same predicted path and same local-only rule.
        BodyCall::SetPlayerBonePose { player, bones } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "SetPlayerBonePose: a client instance may pose only the LOCAL player \
                     ({local:?}), not {player:?}"
                ));
            }
            // Names resolve to rig ids here, exactly as on the server.
            let Some(bones) = crate::modding::resolve_bone_poses(bones) else {
                return HostRet::invalid(crate::modding::BONE_POSE_REFUSAL.into());
            };
            // Latch per BONE, not per body: a mod bending an arm owns that
            // arm locally, but must not blank an unrelated bone another pack
            // is bending server-side.
            let keys: Vec<u16> = bones.iter().map(|b| b.bone).collect();
            if !client.body.set_bone_poses(&mod_id, bones) {
                return HostRet::invalid(crate::modding::BONE_POSE_REFUSAL.into());
            }
            client.poses_bones.extend(keys);
            HostRet::Bool(true)
        }
        // The PREDICTED twin of the server's `HoldUse`: a client has one
        // addressable body, so the only question is whether this mod is taking
        // its press.
        BodyCall::HoldUse { player } => {
            let local = super::scope::active_actor().and_then(|a| a.id);
            if local != Some(player) {
                return HostRet::invalid(format!(
                    "HoldUse: a client instance may take only the LOCAL player's gesture \
                     ({local:?}), not {player:?}"
                ));
            }
            client.holds_use = true;
            HostRet::Bool(true)
        }
    }
}
