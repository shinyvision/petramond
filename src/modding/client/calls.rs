//! The client-instance host-call handler: every `HostCall::Client*`
//! variant, size/namespace-capped, plus the read-only replica scope.

#[cfg(test)]
mod tests;
mod validate;

use std::sync::Arc;

use mod_api::{HostCall, HostRet, RuntimeSide};

use crate::modding::host::guards::key_owned_by_namespace;
use crate::modding::host::{ModStoreData, Phase};

use super::scope as client_scope;
use super::state::{ClientCommand, ClientImageData, ClientOverlayRegistration};
use validate::{
    client_canvas_element_image_key, client_canvas_element_valid, valid_client_key,
    valid_client_key_id, CLIENT_AMBIENT_WIND_MAX, CLIENT_BLOCKS_QUERY_MAX,
    CLIENT_CANVAS_ELEMENT_MAX, CLIENT_CANVAS_MAX,
    CLIENT_CANVAS_SIDE_MAX, CLIENT_COMMAND_MAX, CLIENT_ENV_PARAM_MAX, CLIENT_IMAGE_MAX,
    CLIENT_IMAGE_SIDE_MAX, CLIENT_KEY_BINDING_MAX, CLIENT_OVERLAY_DISPLAY_SIDE_MAX,
    CLIENT_OVERLAY_MAX, CLIENT_SURFACE_QUERY_MAX, CLIENT_TEXT_BYTES_MAX, CLIENT_TEXT_RUN_MAX,
    CLIENT_TEXT_SCALE_MAX, CLIENT_UI_STATE_MAX, CLIENT_UI_STRING_MAX,
};

/// Whether a `client_wasm` instance may issue this call.
///
/// EXHAUSTIVE on purpose — no wildcard arm: appending a `HostCall` variant
/// does not compile until its side is decided here, next to the client
/// handler. Client instances are presentation-only; anything that reaches the
/// simulation, the registries, or the tick scheduler stays `false`.
pub(in crate::modding) fn client_capability(call: &HostCall) -> bool {
    match call {
        // Instance-neutral basics. The whole REGISTRY domain (resolvers, the
        // reverse name lookups, tag membership, item row reads) touches only
        // the process-wide registries, so it is legal on ANY instance, like
        // worldgen workers — a client mod interpreting `ClientBlocksAt` ids
        // resolves the names, tag sets, and rows it compares against the
        // same way the server side does.
        HostCall::Log { .. }
        | HostCall::RuntimeSide
        | HostCall::RngU64 { .. }
        | HostCall::ResolveBlock { .. }
        | HostCall::ResolveItem { .. }
        | HostCall::ResolveMob { .. }
        | HostCall::BlockNames { .. }
        | HostCall::ItemNames { .. }
        | HostCall::MobNames { .. }
        | HostCall::BlocksByTag { .. }
        | HostCall::ItemsByTag { .. }
        | HostCall::ItemInfo { .. } => true,
        // The client presentation surface (handled below).
        HostCall::ClientRegisterOverlay { .. }
        | HostCall::ClientRegisterKey { .. }
        | HostCall::ClientSurfaceColumns { .. }
        | HostCall::ClientUiStateSet { .. }
        | HostCall::ClientUiStateGet { .. }
        | HostCall::ClientImageSet { .. }
        | HostCall::ClientImageBlit { .. }
        | HostCall::ClientTextMeasure { .. }
        | HostCall::ClientImageDrawTexts { .. }
        | HostCall::ClientGuiOpen { .. }
        | HostCall::ClientGuiClose
        | HostCall::ClientCanvasOpen { .. }
        | HostCall::ClientCanvasClose
        | HostCall::ClientCanvasSceneSet { .. }
        | HostCall::ClientCanvasViewSet { .. }
        | HostCall::ClientStorageGetMany { .. }
        | HostCall::ClientStorageSetMany { .. }
        | HostCall::ClientStorageReadBegin { .. }
        | HostCall::ClientStorageReadPoll { .. }
        | HostCall::ClientEnvParams { .. }
        | HostCall::ClientBiomeAt { .. }
        | HostCall::ClientAmbientSet { .. }
        | HostCall::ClientLoopSet { .. }
        | HostCall::ClientMoodSet { .. }
        | HostCall::ClientBlocksAt { .. } => true,
        // Simulation, registration, and registry surfaces: server-side only.
        HostCall::CurrentTick
        | HostCall::RegisterTickSystem { .. }
        | HostCall::RegisterEventHandler { .. }
        | HostCall::GetBlock { .. }
        | HostCall::GetBlocks { .. }
        | HostCall::SetBlock { .. }
        | HostCall::SetBlocks { .. }
        | HostCall::ScheduleTick { .. }
        | HostCall::IsLoaded { .. }
        | HostCall::LightAt { .. }
        | HostCall::SpawnMob { .. }
        | HostCall::MobsInRadius { .. }
        | HostCall::DamageMob { .. }
        | HostCall::DespawnMob { .. }
        | HostCall::SpawnItem { .. }
        | HostCall::PlayerState
        | HostCall::DamagePlayer { .. }
        | HostCall::ApplyKnockback { .. }
        | HostCall::GiveItem { .. }
        | HostCall::SetHealth { .. }
        | HostCall::Teleport { .. }
        | HostCall::EmitSound { .. }
        | HostCall::WorldKvGet { .. }
        | HostCall::WorldKvSet { .. }
        | HostCall::WorldKvDelete { .. }
        | HostCall::SectionKvGet { .. }
        | HostCall::SectionKvSet { .. }
        | HostCall::SectionKvDelete { .. }
        | HostCall::MobKvGet { .. }
        | HostCall::MobKvSet { .. }
        | HostCall::MobKvDelete { .. }
        | HostCall::RegisterWorldgenFeature { .. }
        | HostCall::RegisterStageReplacement { .. }
        | HostCall::RegisterGenerator { .. }
        | HostCall::GuiStateSet { .. }
        | HostCall::GuiStateGet { .. }
        | HostCall::GuiOpen { .. }
        | HostCall::GuiClose
        | HostCall::ChatSend { .. }
        | HostCall::SoundPlayAt { .. }
        | HostCall::SoundPlayOnMob { .. }
        | HostCall::SoundStop { .. }
        | HostCall::CollisionShapeAt { .. }
        | HostCall::ShaderSetParam { .. }
        | HostCall::RegisterHostileSpawner { .. }
        | HostCall::RegisterBlockBehavior { .. }
        | HostCall::RegisterAiNode { .. }
        | HostCall::ContainerGet { .. }
        | HostCall::ContainerSet { .. }
        | HostCall::RecipeResult { .. }
        | HostCall::EffectApply { .. }
        | HostCall::EffectsActive
        | HostCall::SwapModelBlock { .. }
        | HostCall::ContainerGetMany { .. }
        | HostCall::MobEmitterSet { .. }
        | HostCall::MobAnimSet { .. }
        | HostCall::MobAnimRate { .. }
        | HostCall::MobAnimSeek { .. }
        | HostCall::MobAnimState { .. }
        | HostCall::MobDrive { .. }
        | HostCall::MobMount { .. }
        | HostCall::MobDismount { .. }
        | HostCall::MobRiders { .. }
        | HostCall::ConsumeHeld { .. }
        | HostCall::ReplaceHeldOne { .. }
        | HostCall::PlayerInput { .. }
        | HostCall::EmitterBurst { .. }
        | HostCall::BiomeAt { .. }
        | HostCall::SurfaceYAt { .. }
        | HostCall::Players => false,
    }
}

pub(in crate::modding) fn handle_client_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    if data.side != RuntimeSide::Client {
        return HostRet::Error("client host calls require a client_wasm instance".into());
    }
    let mod_id = data.mod_id.clone();
    let Some(client) = data.client.as_mut() else {
        return HostRet::Error("client instance has no client state".into());
    };
    match call {
        HostCall::ClientRegisterOverlay {
            image_key,
            anchor,
            margin,
            display_size,
        } => {
            if data.phase != Phase::Init {
                return HostRet::Error(
                    "client overlays may only be registered during mod_init".into(),
                );
            }
            if !key_owned_by_namespace(&mod_id, &image_key) {
                return HostRet::Error(format!(
                    "client overlay image '{image_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if display_size[0] == 0
                || display_size[1] == 0
                || display_size[0] > CLIENT_OVERLAY_DISPLAY_SIDE_MAX
                || display_size[1] > CLIENT_OVERLAY_DISPLAY_SIDE_MAX
            {
                return HostRet::Error(format!(
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
                return HostRet::Error("client overlay registration limit reached".into());
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
        HostCall::ClientRegisterKey {
            id,
            label,
            key,
            action_id,
        } => {
            if data.phase != Phase::Init {
                return HostRet::Error("client keys may only be registered during mod_init".into());
            }
            if !valid_client_key_id(&id) {
                return HostRet::Error(format!(
                    "invalid client key id '{id}' (bare lowercase snake_case, max 48 chars)"
                ));
            }
            if label.trim().is_empty() || label.len() > 48 {
                return HostRet::Error(format!("invalid client key label '{label}'"));
            }
            if !valid_client_key(&key) {
                return HostRet::Error(format!("unsupported client key '{key}'"));
            }
            if client.key_bindings.iter().any(|b| b.id == id) {
                return HostRet::Error(format!("client key id '{id}' registered twice"));
            }
            if client.key_bindings.len() >= CLIENT_KEY_BINDING_MAX {
                return HostRet::Error("client key registration limit reached".into());
            }
            client.key_bindings.push(super::state::ClientKeyBinding {
                id,
                label,
                key,
                action_id,
            });
            HostRet::Unit
        }
        HostCall::ClientEnvParams { keys } => {
            if keys.len() > CLIENT_ENV_PARAM_MAX {
                return HostRet::Error(format!(
                    "ClientEnvParams key count {} exceeds {CLIENT_ENV_PARAM_MAX}",
                    keys.len()
                ));
            }
            client_scope::with_active(|world| {
                let params = world.environment().shader_params().clone();
                HostRet::EnvParams(keys.iter().map(|k| params.get(k).copied()).collect())
            })
            .unwrap_or_else(|| HostRet::Error("no client replica is active".into()))
        }
        HostCall::ClientBiomeAt { pos } => client_scope::with_active(|world| {
            HostRet::MaybeByte(world.biome_at_world(pos[0], pos[1]))
        })
        .unwrap_or_else(|| HostRet::Error("no client replica is active".into())),
        HostCall::ClientBlocksAt { positions } => {
            if positions.len() > CLIENT_BLOCKS_QUERY_MAX {
                return HostRet::Error(format!(
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
            .unwrap_or_else(|| HostRet::Error("no client replica is active".into()))
        }
        HostCall::ClientAmbientSet {
            key,
            intensity,
            wind,
        } => {
            if !intensity.is_finite()
                || !wind
                    .iter()
                    .all(|w| w.is_finite() && w.abs() <= CLIENT_AMBIENT_WIND_MAX)
            {
                return HostRet::Error(
                    "ClientAmbientSet: intensity and wind must be finite (|wind| ≤ 64)".into(),
                );
            }
            // Unknown keys and non-ambient bundles are forgiving `false`
            // (a disabled pack's bundle is not a protocol break).
            let Some(bundle) = crate::particle_emitters::by_key(&key) else {
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
        HostCall::ClientLoopSet { key, gain } => {
            if !gain.is_finite() {
                return HostRet::Error("ClientLoopSet: gain must be finite".into());
            }
            let Some(sound) = crate::audio::sound_by_name(&key) else {
                return HostRet::Bool(false);
            };
            client.sound_loops.insert(sound, gain.clamp(0.0, 4.0));
            HostRet::Bool(true)
        }
        HostCall::ClientMoodSet { darken, desaturate } => {
            if !darken.is_finite() || !desaturate.is_finite() {
                return HostRet::Error("ClientMoodSet: values must be finite".into());
            }
            // The clamp IS the safety contract: no mod can black the screen
            // out; it can only be moody about it.
            client.mood = [darken.clamp(0.0, 0.5), desaturate.clamp(0.0, 0.5)];
            HostRet::Bool(true)
        }
        HostCall::ClientSurfaceColumns { queries } => {
            if queries.len() > CLIENT_SURFACE_QUERY_MAX {
                return HostRet::Error(format!(
                    "ClientSurfaceColumns query count {} exceeds {CLIENT_SURFACE_QUERY_MAX}",
                    queries.len()
                ));
            }
            client_scope::with_active(|world| {
                let mut cells = [None::<(i16, [u8; 3])>; 256];
                let columns = queries
                    .iter()
                    .map(|query| {
                        let pos = crate::chunk::ChunkPos::new(query.coord[0], query.coord[1]);
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
            .unwrap_or_else(|| HostRet::Error("no client replica is active".into()))
        }
        HostCall::ClientUiStateSet { key, value } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::Error(format!(
                    "client UI key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if matches!(&value, mod_api::GuiValue::Str(text) if text.len() > CLIENT_UI_STRING_MAX) {
                return HostRet::Error("client UI string exceeds its size limit".into());
            }
            if !client.ui_state.contains_key(&key) && client.ui_state.len() >= CLIENT_UI_STATE_MAX {
                return HostRet::Error("client UI state entry limit reached".into());
            }
            Arc::make_mut(&mut client.ui_state).insert(key, value);
            HostRet::Unit
        }
        HostCall::ClientUiStateGet { key } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::Error(format!(
                    "client UI key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            HostRet::GuiValue(client.ui_state.get(&key).cloned())
        }
        HostCall::ClientImageSet {
            key,
            width,
            height,
            rgba,
        } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::Error(format!(
                    "client image key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if width == 0
                || height == 0
                || width > CLIENT_IMAGE_SIDE_MAX
                || height > CLIENT_IMAGE_SIDE_MAX
                || rgba.len() != width as usize * height as usize * 4
            {
                return HostRet::Error(format!(
                    "invalid client image {width}x{height} with {} RGBA bytes",
                    rgba.len()
                ));
            }
            if !client.images.contains_key(&key) && client.images.len() >= CLIENT_IMAGE_MAX {
                return HostRet::Error("client image limit reached".into());
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
        HostCall::ClientImageBlit {
            key,
            origin,
            size,
            rgba,
        } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::Error(format!(
                    "client image key '{key}' must be namespaced '{mod_id}:name'"
                ));
            }
            let Some((width, height)) = client
                .images
                .get(&key)
                .map(|image| (image.width as usize, image.height as usize))
            else {
                return HostRet::Error(format!("client image '{key}' has not been published"));
            };
            let (w, h) = (size[0] as usize, size[1] as usize);
            if w == 0
                || h == 0
                || origin[0] as usize + w > width
                || origin[1] as usize + h > height
                || rgba.len() != w * h * 4
            {
                return HostRet::Error(format!(
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
        HostCall::ClientTextMeasure { text, scale } => {
            if scale == 0 || scale > CLIENT_TEXT_SCALE_MAX {
                return HostRet::Error(format!(
                    "client text scale {scale} must be 1..={CLIENT_TEXT_SCALE_MAX}"
                ));
            }
            if text.len() > CLIENT_TEXT_BYTES_MAX || text.contains(['\n', '\r']) {
                return HostRet::Error("invalid single-line client text".into());
            }
            let [width, height] = petramond_text::measure_scaled(&text, scale);
            let Ok(width) = u16::try_from(width) else {
                return HostRet::Error("client text width exceeds u16".into());
            };
            let Ok(height) = u16::try_from(height) else {
                return HostRet::Error("client text height exceeds u16".into());
            };
            HostRet::ClientTextSize([width, height])
        }
        HostCall::ClientImageDrawTexts { key, runs } => {
            if !key_owned_by_namespace(&mod_id, &key) {
                return HostRet::Error(format!(
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
                return HostRet::Error("invalid client text run batch".into());
            }
            if !client.images.contains_key(&key) {
                return HostRet::Error(format!("client image '{key}' has not been published"));
            }
            let revision = client.next_image_revision;
            client.next_image_revision = client.next_image_revision.wrapping_add(1).max(1);
            let image = client.images.get_mut(&key).unwrap();
            let rgba = Arc::make_mut(&mut image.rgba);
            for run in runs {
                petramond_text::draw_rgba(
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
        HostCall::ClientGuiOpen { kind_key } => {
            if !key_owned_by_namespace(&mod_id, &kind_key) {
                return HostRet::Error(format!(
                    "client GUI kind '{kind_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::Error("client GUI command queue limit reached".into());
            }
            client.commands.push(ClientCommand::OpenGui {
                owner: mod_id,
                kind: kind_key,
            });
            HostRet::Bool(true)
        }
        HostCall::ClientGuiClose => {
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::Error("client GUI command queue limit reached".into());
            }
            client
                .commands
                .push(ClientCommand::CloseGui { owner: mod_id });
            HostRet::Unit
        }
        HostCall::ClientCanvasOpen { canvas_key, size } => {
            if !key_owned_by_namespace(&mod_id, &canvas_key) {
                return HostRet::Error(format!(
                    "client canvas '{canvas_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if size[0] == 0
                || size[1] == 0
                || size[0] > CLIENT_CANVAS_SIDE_MAX
                || size[1] > CLIENT_CANVAS_SIDE_MAX
            {
                return HostRet::Error(format!(
                    "invalid client canvas size {}x{}",
                    size[0], size[1]
                ));
            }
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::Error("client command queue limit reached".into());
            }
            client.commands.push(ClientCommand::OpenCanvas {
                owner: mod_id,
                canvas_key,
                size,
            });
            HostRet::Bool(true)
        }
        HostCall::ClientCanvasClose => {
            if client.commands.len() >= CLIENT_COMMAND_MAX {
                return HostRet::Error("client command queue limit reached".into());
            }
            client
                .commands
                .push(ClientCommand::CloseCanvas { owner: mod_id });
            HostRet::Unit
        }
        HostCall::ClientCanvasSceneSet {
            canvas_key,
            elements,
        } => {
            if !key_owned_by_namespace(&mod_id, &canvas_key) {
                return HostRet::Error(format!(
                    "client canvas key '{canvas_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if elements.len() > CLIENT_CANVAS_ELEMENT_MAX {
                return HostRet::Error("client canvas element limit reached".into());
            }
            if let Some(image_key) = elements
                .iter()
                .map(client_canvas_element_image_key)
                .find(|key| !key_owned_by_namespace(&mod_id, key))
            {
                return HostRet::Error(format!(
                    "client canvas image '{}' must be namespaced '{mod_id}:name'",
                    image_key
                ));
            }
            if elements
                .iter()
                .any(|element| !client_canvas_element_valid(element))
            {
                return HostRet::Error("client canvas element geometry is invalid".into());
            }
            if !client.canvas_scenes.contains_key(&canvas_key)
                && client.canvas_scenes.len() >= CLIENT_CANVAS_MAX
            {
                return HostRet::Error("client canvas limit reached".into());
            }
            client.canvas_scenes.entry(canvas_key).or_default().elements = elements;
            HostRet::Unit
        }
        HostCall::ClientCanvasViewSet { canvas_key, offset } => {
            if !key_owned_by_namespace(&mod_id, &canvas_key) {
                return HostRet::Error(format!(
                    "client canvas key '{canvas_key}' must be namespaced '{mod_id}:name'"
                ));
            }
            if !offset[0].is_finite() || !offset[1].is_finite() {
                return HostRet::Error("client canvas view offset must be finite".into());
            }
            if !client.canvas_scenes.contains_key(&canvas_key)
                && client.canvas_scenes.len() >= CLIENT_CANVAS_MAX
            {
                return HostRet::Error("client canvas limit reached".into());
            }
            client.canvas_scenes.entry(canvas_key).or_default().offset = offset;
            HostRet::Unit
        }
        HostCall::ClientStorageGetMany { keys } => {
            for key in &keys {
                if !key_owned_by_namespace(&mod_id, key) {
                    return HostRet::Error(format!(
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
                Err(error) => HostRet::Error(error),
            }
        }
        HostCall::ClientStorageReadBegin { keys } => {
            for key in &keys {
                if !key_owned_by_namespace(&mod_id, key) {
                    return HostRet::Error(format!(
                        "client storage key '{key}' must be namespaced '{mod_id}:name'"
                    ));
                }
            }
            match client.storage.read_begin(keys) {
                Ok(ticket) => HostRet::U64(ticket),
                Err(error) => HostRet::Error(error),
            }
        }
        HostCall::ClientStorageReadPoll { ticket } => match client.storage.read_poll(ticket) {
            Ok(values) => HostRet::ClientStorageRead(values.map(|values| {
                values
                    .into_iter()
                    .map(|value| value.map(mod_api::ByteBuf::from))
                    .collect()
            })),
            Err(error) => HostRet::Error(error),
        },
        HostCall::ClientStorageSetMany { entries } => {
            for (key, value) in &entries {
                if !key_owned_by_namespace(&mod_id, key) {
                    return HostRet::Error(format!(
                        "client storage key '{key}' must be namespaced '{mod_id}:name'"
                    ));
                }
                if key.len() > super::storage::KEY_MAX || value.len() > super::storage::VALUE_MAX {
                    return HostRet::Error(format!(
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
                Err(error) => HostRet::Error(error),
            }
        }
        other => HostRet::Error(format!(
            "non-client call {other:?} mis-routed to handle_client_call (host bug)"
        )),
    }
}
