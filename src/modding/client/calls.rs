//! The client-instance host-call handler: every `HostCall::Client*`
//! variant, size/namespace-capped, plus the read-only replica scope.

use std::sync::Arc;

use mod_api::{HostCall, HostRet, RuntimeSide};

use crate::modding::host::guards::key_owned_by_namespace;
use crate::modding::host::{ModStoreData, Phase};

use super::scope as client_scope;
use super::state::{ClientCommand, ClientImageData, ClientOverlayRegistration};

fn client_canvas_element_image_key(element: &mod_api::ClientCanvasElement) -> &str {
    match element {
        mod_api::ClientCanvasElement::Image { image_key, .. }
        | mod_api::ClientCanvasElement::Sprite { image_key, .. } => image_key,
    }
}

fn client_canvas_element_valid(element: &mod_api::ClientCanvasElement) -> bool {
    match element {
        mod_api::ClientCanvasElement::Image { rect, .. } => {
            rect.iter().all(|value| value.is_finite()) && rect[2] > 0.0 && rect[3] > 0.0
        }
        mod_api::ClientCanvasElement::Sprite { center, .. } => {
            center[0].is_finite() && center[1].is_finite()
        }
    }
}

const CLIENT_SURFACE_QUERY_MAX: usize = 512;
const CLIENT_IMAGE_SIDE_MAX: u16 = 640;
const CLIENT_OVERLAY_MAX: usize = 16;
const CLIENT_OVERLAY_DISPLAY_SIDE_MAX: u16 = 2048;
const CLIENT_KEY_BINDING_MAX: usize = 32;
const CLIENT_UI_STATE_MAX: usize = 1024;
const CLIENT_UI_STRING_MAX: usize = 16 << 10;
const CLIENT_IMAGE_MAX: usize = 64;
const CLIENT_TEXT_RUN_MAX: usize = 256;
const CLIENT_TEXT_BYTES_MAX: usize = 16 << 10;
const CLIENT_TEXT_SCALE_MAX: u8 = 8;
const CLIENT_COMMAND_MAX: usize = 64;
const CLIENT_CANVAS_SIDE_MAX: u16 = 2048;
const CLIENT_CANVAS_MAX: usize = 8;
const CLIENT_CANVAS_ELEMENT_MAX: usize = 64;
/// Named shader params one `ClientEnvParams` call may read (the GPU slot
/// budget — no shader can consume more anyway).
const CLIENT_ENV_PARAM_MAX: usize = 16;
/// Largest per-axis ambient wind magnitude, blocks/s.
const CLIENT_AMBIENT_WIND_MAX: f32 = 64.0;

/// Whether a `client_wasm` instance may issue this call.
///
/// EXHAUSTIVE on purpose — no wildcard arm: appending a `HostCall` variant
/// does not compile until its side is decided here, next to the client
/// handler. Client instances are presentation-only; anything that reaches the
/// simulation, the registries, or the tick scheduler stays `false`.
pub(in crate::modding) fn client_capability(call: &HostCall) -> bool {
    match call {
        // Instance-neutral basics.
        HostCall::Log { .. } | HostCall::RuntimeSide | HostCall::RngU64 { .. } => true,
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
        | HostCall::ClientMoodSet { .. } => true,
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
        | HostCall::SpawnMobChecked { .. }
        | HostCall::MobsInRadius { .. }
        | HostCall::DamageMob { .. }
        | HostCall::DespawnMob { .. }
        | HostCall::SpawnItem { .. }
        | HostCall::PlayerState
        | HostCall::DamagePlayer { .. }
        | HostCall::ApplyKnockback { .. }
        | HostCall::GiveItem { .. }
        | HostCall::KillPlayer
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
        | HostCall::ResolveBlock { .. }
        | HostCall::ResolveItem { .. }
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
        | HostCall::BlockIsFullSpawnSupport { .. }
        | HostCall::ShaderSetParam { .. }
        | HostCall::RegisterHostileSpawner { .. }
        | HostCall::RegisterBlockBehavior { .. }
        | HostCall::RegisterAiNode { .. }
        | HostCall::ContainerGet { .. }
        | HostCall::ContainerSet { .. }
        | HostCall::ItemInfo { .. }
        | HostCall::RecipeResult { .. }
        | HostCall::EffectApply { .. }
        | HostCall::EffectRemove { .. }
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

fn valid_client_key(key: &str) -> bool {
    key.strip_prefix("key_")
        .is_some_and(|tail| tail.len() == 1 && tail.as_bytes()[0].is_ascii_lowercase())
        || key
            .strip_prefix("digit_")
            .is_some_and(|tail| tail.len() == 1 && tail.as_bytes()[0].is_ascii_digit())
}

/// A bare (un-namespaced) action id: lowercase snake_case, persisted in the
/// player's client.json as `mod_id:id` — so it must be stable and file-safe.
fn valid_client_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet, RuntimeSide};

    use crate::modding::host::{handle_host_call, ModStoreData};

    fn client_data(tag: &str) -> ModStoreData {
        ModStoreData::new_for_side(
            "weathertest",
            7,
            RuntimeSide::Client,
            Some(std::env::temp_dir().join(format!("petramond-client-calls-{tag}"))),
        )
    }

    /// The weather-era client calls: unknown keys are FORGIVING `false`
    /// (a disabled pack is not a protocol break), malformed values are hard
    /// errors, and the env-param read is capped at the GPU slot budget.
    #[test]
    fn weather_era_client_calls_validate_and_forgive() {
        let mut data = client_data("weather-era");
        // Unknown bundle key / unknown sound key: forgiving false.
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientAmbientSet {
                    key: "nope:rain".into(),
                    intensity: 1.0,
                    wind: [0.0, 0.0],
                },
            ),
            HostRet::Bool(false)
        );
        // A real bundle that is NOT ambient (the engine water splash burst):
        // also forgiving false.
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientAmbientSet {
                    key: crate::particle_emitters::WATER_SPLASH_KEY.into(),
                    intensity: 1.0,
                    wind: [0.0, 0.0],
                },
            ),
            HostRet::Bool(false)
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientLoopSet {
                    key: "nope:loop".into(),
                    gain: 1.0,
                },
            ),
            HostRet::Bool(false)
        );
        // Non-finite / out-of-envelope values are hard errors.
        for bad in [
            HostCall::ClientAmbientSet {
                key: "m:x".into(),
                intensity: f32::NAN,
                wind: [0.0, 0.0],
            },
            HostCall::ClientAmbientSet {
                key: "m:x".into(),
                intensity: 1.0,
                wind: [65.0, 0.0],
            },
            HostCall::ClientLoopSet {
                key: "m:x".into(),
                gain: f32::INFINITY,
            },
            HostCall::ClientMoodSet {
                darken: f32::NAN,
                desaturate: 0.0,
            },
        ] {
            let ret = handle_host_call(&mut data, bad.clone());
            assert!(
                matches!(ret, HostRet::Error(_)),
                "malformed values must be a hard error: {bad:?} -> {ret:?}"
            );
        }
        // The mood clamps into its subtle envelope and always succeeds.
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientMoodSet {
                    darken: 9.0,
                    desaturate: -3.0,
                },
            ),
            HostRet::Bool(true)
        );
        assert_eq!(data.client.as_ref().unwrap().mood, [0.5, 0.0]);
        // Env-param reads cap at the 16-slot GPU budget.
        assert!(matches!(
            handle_host_call(
                &mut data,
                HostCall::ClientEnvParams {
                    keys: (0..17).map(|i| format!("m:k{i}")).collect(),
                },
            ),
            HostRet::Error(_)
        ));
    }

    /// The weather-era SERVER calls are rejected on a client instance by the
    /// capability gate, like every sim-facing call.
    #[test]
    fn weather_era_server_calls_stay_server_side() {
        let mut data = client_data("server-side");
        for call in [
            HostCall::BiomeAt { pos: [0, 0] },
            HostCall::SurfaceYAt { pos: [0, 0] },
            HostCall::Players,
        ] {
            assert!(
                matches!(handle_host_call(&mut data, call), HostRet::Error(_)),
                "sim queries must be rejected on client instances"
            );
        }
    }

    #[test]
    fn client_instances_are_capability_isolated_and_namespace_their_state() {
        let mut data = ModStoreData::new_for_side(
            "map",
            7,
            RuntimeSide::Client,
            Some(std::env::temp_dir().join("petramond-unused-client-mod-test")),
        );
        assert_eq!(
            handle_host_call(&mut data, HostCall::RuntimeSide),
            HostRet::RuntimeSide(RuntimeSide::Client)
        );
        assert!(matches!(
            handle_host_call(
                &mut data,
                HostCall::RegisterTickSystem {
                    stage: mod_api::Stage::Mobs,
                    attach: mod_api::AttachSide::After,
                    priority: 0,
                    system_id: 1,
                }
            ),
            HostRet::Error(_)
        ));
        assert!(data.pending.is_empty());
        assert!(matches!(
            handle_host_call(
                &mut data,
                HostCall::ClientUiStateSet {
                    key: "other:value".into(),
                    value: mod_api::GuiValue::I32(1),
                }
            ),
            HostRet::Error(_)
        ));
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientUiStateSet {
                    key: "map:value".into(),
                    value: mod_api::GuiValue::I32(2),
                }
            ),
            HostRet::Unit
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientUiStateGet {
                    key: "map:value".into(),
                }
            ),
            HostRet::GuiValue(Some(mod_api::GuiValue::I32(2)))
        );

        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientImageSet {
                    key: "map:tile".into(),
                    width: 1,
                    height: 1,
                    rgba: vec![1, 2, 3, 255],
                },
            ),
            HostRet::Unit
        );
        let image_revision = data
            .client
            .as_ref()
            .unwrap()
            .images
            .get("map:tile")
            .unwrap()
            .revision;
        let elements = vec![mod_api::ClientCanvasElement::Image {
            image_key: "map:tile".into(),
            rect: [0.0, 0.0, 160.0, 160.0],
        }];
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientCanvasSceneSet {
                    canvas_key: "map:canvas".into(),
                    elements: elements.clone(),
                },
            ),
            HostRet::Unit
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientCanvasViewSet {
                    canvas_key: "map:canvas".into(),
                    offset: [12.0, -7.0],
                },
            ),
            HostRet::Unit
        );
        let client = data.client.as_ref().unwrap();
        let scene = client.canvas_scenes.get("map:canvas").unwrap();
        assert_eq!(scene.elements, elements);
        assert_eq!(scene.offset, [12.0, -7.0]);
        assert_eq!(client.images["map:tile"].revision, image_revision);
    }

    #[test]
    fn client_image_blit_mutates_in_place_and_validates_bounds() {
        let mut data = ModStoreData::new_for_side(
            "map",
            7,
            RuntimeSide::Client,
            Some(std::env::temp_dir().join("petramond-unused-client-blit-test")),
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientImageSet {
                    key: "map:tile".into(),
                    width: 2,
                    height: 2,
                    rgba: vec![0; 16],
                },
            ),
            HostRet::Unit
        );
        let revision = data.client.as_ref().unwrap().images["map:tile"].revision;
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ClientImageBlit {
                    key: "map:tile".into(),
                    origin: [1, 1],
                    size: [1, 1],
                    rgba: vec![9, 8, 7, 255],
                },
            ),
            HostRet::Unit
        );
        let image = &data.client.as_ref().unwrap().images["map:tile"];
        assert_eq!(&image.rgba[12..16], &[9, 8, 7, 255], "blit lands at (1,1)");
        assert_eq!(&image.rgba[0..4], &[0, 0, 0, 0], "pixels outside stay");
        assert_ne!(image.revision, revision, "a blit must move the revision");
        assert_eq!(
            image.recent_blits,
            vec![(image.revision, [1, 1, 1, 1])],
            "the blit records its rect for partial texture uploads"
        );

        // The partial-update chain: bounded window, oldest first, broken by
        // whole-image mutations (text draws, re-publish).
        for _ in 0..super::super::state::IMAGE_BLIT_WINDOW + 2 {
            handle_host_call(
                &mut data,
                HostCall::ClientImageBlit {
                    key: "map:tile".into(),
                    origin: [0, 0],
                    size: [1, 1],
                    rgba: vec![1, 1, 1, 255],
                },
            );
        }
        let image = &data.client.as_ref().unwrap().images["map:tile"];
        assert_eq!(
            image.recent_blits.len(),
            super::super::state::IMAGE_BLIT_WINDOW
        );
        assert!(
            image.recent_blits.windows(2).all(|w| w[1].0 == w[0].0 + 1),
            "window entries stay consecutive"
        );
        assert_eq!(image.recent_blits.last().unwrap().0, image.revision);
        handle_host_call(
            &mut data,
            HostCall::ClientImageDrawTexts {
                key: "map:tile".into(),
                runs: vec![mod_api::ClientTextRun {
                    text: "x".into(),
                    position: [0, 0],
                    scale: 1,
                    color: [255, 255, 255, 255],
                }],
            },
        );
        let image = &data.client.as_ref().unwrap().images["map:tile"];
        assert!(
            image.recent_blits.is_empty(),
            "text draws break the partial chain (no rect is tracked for them)"
        );

        for bad in [
            // out of bounds
            HostCall::ClientImageBlit {
                key: "map:tile".into(),
                origin: [2, 0],
                size: [1, 1],
                rgba: vec![0; 4],
            },
            // byte count mismatch
            HostCall::ClientImageBlit {
                key: "map:tile".into(),
                origin: [0, 0],
                size: [1, 1],
                rgba: vec![0; 3],
            },
            // never published
            HostCall::ClientImageBlit {
                key: "map:none".into(),
                origin: [0, 0],
                size: [1, 1],
                rgba: vec![0; 4],
            },
            // foreign namespace
            HostCall::ClientImageBlit {
                key: "other:tile".into(),
                origin: [0, 0],
                size: [1, 1],
                rgba: vec![0; 4],
            },
        ] {
            assert!(matches!(
                handle_host_call(&mut data, bad),
                HostRet::Error(_)
            ));
        }
    }

    #[test]
    fn client_surface_columns_gate_on_revision_and_pack_cells() {
        let mut data = ModStoreData::new_for_side(
            "map",
            7,
            RuntimeSide::Client,
            Some(std::env::temp_dir().join("petramond-unused-client-surface-test")),
        );
        let mut world = crate::world::World::new(0, 0);
        let sp = crate::chunk::SectionPos::new(0, 4, 0);
        world.insert_section_for_test(sp, crate::section::Section::new(0, 4, 0));
        assert!(world.set_block_world(3, 64, 5, crate::block::Block::Stone));

        let query = |revision| HostCall::ClientSurfaceColumns {
            queries: vec![
                mod_api::ClientSurfaceQuery {
                    coord: [0, 0],
                    revision,
                },
                mod_api::ClientSurfaceQuery {
                    coord: [9, 9],
                    revision: 0,
                },
            ],
        };
        let HostRet::ClientSurfaceColumns(replies) =
            super::client_scope::enter(&world, || handle_host_call(&mut data, query(0)))
        else {
            panic!("surface columns reply expected");
        };
        assert!(replies[1].is_none(), "an unloaded column replies None");
        let column = replies[0].as_ref().expect("loaded column");
        let cells = column.cells.as_ref().expect("first sight sends cells");
        assert_eq!(cells.len(), mod_api::CLIENT_SURFACE_COLUMN_BYTES);
        let cell = |lx: usize, lz: usize| {
            let at = (lz * 16 + lx) * mod_api::CLIENT_SURFACE_CELL_BYTES;
            i16::from_le_bytes([cells[at], cells[at + 1]])
        };
        assert_eq!(cell(3, 5), 64, "the placed surface cell is known");
        assert_eq!(
            cell(0, 0),
            mod_api::CLIENT_SURFACE_UNKNOWN_HEIGHT,
            "cells with no surface stay unknown"
        );

        // Echoing the served revision skips the cell payload…
        let revision = column.revision;
        let HostRet::ClientSurfaceColumns(replies) =
            super::client_scope::enter(&world, || handle_host_call(&mut data, query(revision)))
        else {
            panic!("surface columns reply expected");
        };
        let unchanged = replies[0].as_ref().expect("loaded column");
        assert_eq!(unchanged.revision, revision);
        assert!(unchanged.cells.is_none(), "unchanged column sends no cells");

        // …until an edit moves the column revision.
        assert!(world.set_block_world(3, 64, 5, crate::block::Block::Dirt));
        let HostRet::ClientSurfaceColumns(replies) =
            super::client_scope::enter(&world, || handle_host_call(&mut data, query(revision)))
        else {
            panic!("surface columns reply expected");
        };
        let changed = replies[0].as_ref().expect("loaded column");
        assert_ne!(changed.revision, revision);
        assert!(changed.cells.is_some(), "a moved revision resends cells");
    }
}
