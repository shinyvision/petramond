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

const CLIENT_SURFACE_RADIUS_MAX: u16 = 192;
const CLIENT_IMAGE_SIDE_MAX: u16 = 640;
const CLIENT_OVERLAY_MAX: usize = 16;
const CLIENT_OVERLAY_DISPLAY_SIDE_MAX: u16 = 2048;
const CLIENT_KEY_BINDING_MAX: usize = 32;
const CLIENT_UI_STATE_MAX: usize = 1024;
const CLIENT_UI_STRING_MAX: usize = 16 << 10;
const CLIENT_IMAGE_MAX: usize = 32;
const CLIENT_TEXT_RUN_MAX: usize = 256;
const CLIENT_TEXT_BYTES_MAX: usize = 16 << 10;
const CLIENT_TEXT_SCALE_MAX: u8 = 8;
const CLIENT_COMMAND_MAX: usize = 64;
const CLIENT_CANVAS_SIDE_MAX: u16 = 2048;
const CLIENT_CANVAS_MAX: usize = 8;
const CLIENT_CANVAS_ELEMENT_MAX: usize = 64;

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
        | HostCall::ClientSurface { .. }
        | HostCall::ClientUiStateSet { .. }
        | HostCall::ClientUiStateGet { .. }
        | HostCall::ClientImageSet { .. }
        | HostCall::ClientTextMeasure { .. }
        | HostCall::ClientImageDrawTexts { .. }
        | HostCall::ClientGuiOpen { .. }
        | HostCall::ClientGuiClose
        | HostCall::ClientCanvasOpen { .. }
        | HostCall::ClientCanvasClose
        | HostCall::ClientCanvasSceneSet { .. }
        | HostCall::ClientCanvasViewSet { .. }
        | HostCall::ClientStorageGetMany { .. }
        | HostCall::ClientStorageSetMany { .. } => true,
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
        | HostCall::EmitterBurst { .. } => false,
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
        HostCall::ClientSurface { center, radius } => {
            if radius > CLIENT_SURFACE_RADIUS_MAX {
                return HostRet::Error(format!(
                    "ClientSurface radius {radius} exceeds {CLIENT_SURFACE_RADIUS_MAX}"
                ));
            }
            client_scope::with_active(|world| {
                let side = radius as i32 * 2 + 1;
                let mut cells = Vec::with_capacity((side * side) as usize);
                for dz in -(radius as i32)..=radius as i32 {
                    for dx in -(radius as i32)..=radius as i32 {
                        cells.push(
                            world
                                .client_surface_cell(center[0] + dx, center[1] + dz)
                                .map(|(height, rgb)| mod_api::ClientSurfaceCell { height, rgb }),
                        );
                    }
                }
                HostRet::ClientSurface(cells)
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
                },
            );
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
                Ok(values) => HostRet::ClientStorageValues(values),
                Err(error) => HostRet::Error(error),
            }
        }
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
}
