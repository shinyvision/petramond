//! Presentation-only client WASM instances.
//!
//! A pack opts in with `client_wasm`; this runtime is separate from the
//! deterministic server/worldgen instances. It can sample final cells from
//! the client's replica, publish document state/images, receive registered
//! key, GUI, and canvas events, and persist namespaced blobs in a host-owned sandbox.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use mod_api::{ClientFrameData, ClientUiEvent, GuestCall, GuestRet, RuntimeSide};

use crate::world::World;

use super::state::{ClientCommand, ClientImageData};
use crate::modding::instance::ModInstance;

struct ClientMod {
    id: String,
    instance: ModInstance,
}

#[derive(Clone)]
pub(crate) struct ClientUiView {
    pub state: Arc<std::collections::BTreeMap<String, mod_api::GuiValue>>,
    pub images: Vec<ClientImageData>,
}

pub(crate) struct ClientCanvasElementView {
    pub element: mod_api::ClientCanvasElement,
    pub image: ClientImageData,
}

pub(crate) struct ClientCanvasView {
    pub offset: [f32; 2],
    pub elements: Vec<ClientCanvasElementView>,
}

pub(crate) struct ClientModRuntime {
    mods: Vec<ClientMod>,
    bindings: Vec<(String, usize, u32)>,
    overlays: Vec<super::state::ClientOverlayRegistration>,
    pressed: HashSet<String>,
}

impl ClientModRuntime {
    /// Load the session's client mods. `enabled` is the session's
    /// mod-enablement AUTHORITY: locally the installed packs minus the
    /// world's disabled set; on a remote join the server's
    /// handshake-reported mod set. A locally installed client mod the
    /// server does not run therefore never activates.
    pub(crate) fn load(world_seed: u32, session_key: &str, enabled: &BTreeSet<String>) -> Self {
        let mut mods = Vec::new();
        for (id, path) in session_client_mods(crate::assets::packs(), enabled) {
            let module = match crate::modding::host::module_for(&path) {
                Ok(module) => module,
                Err(e) => {
                    log::error!("client mod '{id}' disabled: {e}");
                    continue;
                }
            };
            let storage = client_storage_dir(session_key, &id);
            let mut instance = match ModInstance::from_module_side(
                &id,
                &module,
                world_seed,
                RuntimeSide::Client,
                Some(storage),
            ) {
                Ok(instance) => instance,
                Err(e) => {
                    log::error!("client mod '{id}' disabled: {e}");
                    continue;
                }
            };
            instance.call_init_detached();
            if instance.disabled() {
                continue;
            }
            // Client registrations live in ClientStoreData; simulation
            // registrations are irrelevant to this isolated instance.
            instance.take_registrations();
            mods.push(ClientMod { id, instance });
        }

        let mut bindings = Vec::new();
        let mut overlays = Vec::new();
        for (index, loaded) in mods.iter().enumerate() {
            let Some(data) = loaded.instance.client_data() else {
                continue;
            };
            // Overlay image keys are namespace-guarded at registration (and
            // per-mod duplicates rejected there), so keys are already unique
            // across mods.
            overlays.extend(data.overlays.iter().cloned());
            for (key, action) in &data.key_bindings {
                if reserved_key(key) {
                    log::error!(
                        "client mod '{}': key '{}' conflicts with an engine binding; ignored",
                        loaded.id,
                        key
                    );
                    continue;
                }
                if bindings.iter().any(|(bound, _, _)| bound == key) {
                    log::error!(
                        "client mod '{}': key '{}' conflicts with an earlier client mod; ignored",
                        loaded.id,
                        key
                    );
                    continue;
                }
                bindings.push((key.clone(), index, *action));
            }
        }
        Self {
            mods,
            bindings,
            overlays,
            pressed: HashSet::new(),
        }
    }

    /// The live (non-disabled) mod owning a namespaced `mod_id:name` key.
    fn owner_mod(&self, key: &str) -> Option<&ClientMod> {
        let owner = key.split_once(':')?.0;
        self.mods
            .iter()
            .find(|loaded| loaded.id == owner && !loaded.instance.disabled())
    }

    /// [`owner_mod`](Self::owner_mod) for dispatching into the owner.
    fn owner_mod_mut(&mut self, key: &str) -> Option<&mut ClientMod> {
        let owner = key.split_once(':')?.0;
        self.mods
            .iter_mut()
            .find(|loaded| loaded.id == owner && !loaded.instance.disabled())
    }

    pub(crate) fn frame(&mut self, world: &World, frame: ClientFrameData) {
        let call = GuestCall::ClientFrame { frame };
        for loaded in &mut self.mods {
            dispatch_unit(&mut loaded.instance, world, &call, "client frame");
        }
    }

    pub(crate) fn key(&mut self, world: &World, key: &str, pressed: bool) -> bool {
        let Some((_, index, action_id)) = self
            .bindings
            .iter()
            .find(|(bound, _, _)| bound == key)
            .cloned()
        else {
            return false;
        };
        if self.mods[index].instance.disabled() {
            self.pressed.remove(key);
            return false;
        }
        let was_pressed = self.pressed.contains(key);
        if was_pressed == pressed {
            return true;
        }
        if pressed {
            self.pressed.insert(key.to_owned());
        } else {
            self.pressed.remove(key);
        }
        let call = GuestCall::ClientKey { action_id, pressed };
        dispatch_unit(&mut self.mods[index].instance, world, &call, "client key");
        true
    }

    pub(crate) fn ui_event(&mut self, world: &World, kind_key: &str, event: ClientUiEvent) {
        let call = GuestCall::ClientUi {
            kind_key: kind_key.to_owned(),
            event,
        };
        let Some(loaded) = self.owner_mod_mut(kind_key) else {
            return;
        };
        dispatch_unit(&mut loaded.instance, world, &call, "client UI event");
    }

    pub(crate) fn canvas_event(
        &mut self,
        world: &World,
        canvas_key: &str,
        event: mod_api::ClientCanvasEvent,
    ) {
        let call = GuestCall::ClientCanvas {
            canvas_key: canvas_key.to_owned(),
            event,
        };
        let Some(loaded) = self.owner_mod_mut(canvas_key) else {
            return;
        };
        dispatch_unit(&mut loaded.instance, world, &call, "client canvas event");
    }

    pub(crate) fn release_all_keys(&mut self, world: &World) {
        let pressed: Vec<_> = self.pressed.drain().collect();
        for key in pressed {
            let Some((_, index, action_id)) = self
                .bindings
                .iter()
                .find(|(bound, _, _)| bound == &key)
                .cloned()
            else {
                continue;
            };
            let call = GuestCall::ClientKey {
                action_id,
                pressed: false,
            };
            dispatch_unit(
                &mut self.mods[index].instance,
                world,
                &call,
                "client key release",
            );
        }
    }

    pub(crate) fn overlays(&self) -> &[super::state::ClientOverlayRegistration] {
        &self.overlays
    }

    pub(crate) fn image(&self, image_key: &str) -> Option<ClientImageData> {
        self.owner_mod(image_key)?
            .instance
            .client_data()?
            .images
            .get(image_key)
            .cloned()
    }

    pub(crate) fn canvas_view(&self, canvas_key: &str) -> Option<ClientCanvasView> {
        let data = self.owner_mod(canvas_key)?.instance.client_data()?;
        let scene = data.canvas_scenes.get(canvas_key)?;
        let elements = scene
            .elements
            .iter()
            .filter_map(|element| {
                let image_key = match element {
                    mod_api::ClientCanvasElement::Image { image_key, .. }
                    | mod_api::ClientCanvasElement::Sprite { image_key, .. } => image_key,
                };
                data.images
                    .get(image_key)
                    .cloned()
                    .map(|image| ClientCanvasElementView {
                        element: element.clone(),
                        image,
                    })
            })
            .collect();
        Some(ClientCanvasView {
            offset: scene.offset,
            elements,
        })
    }

    pub(crate) fn view_for(&self, kind_key: &str) -> Option<ClientUiView> {
        let data = self.owner_mod(kind_key)?.instance.client_data()?;
        Some(ClientUiView {
            state: data.ui_state.clone(),
            images: data.images.values().cloned().collect(),
        })
    }

    pub(crate) fn take_commands(&mut self) -> Vec<ClientCommand> {
        let mut out = Vec::new();
        for loaded in &mut self.mods {
            if loaded.instance.disabled() {
                if let Some(data) = loaded.instance.client_data_mut() {
                    data.commands.clear();
                }
                continue;
            }
            if let Some(data) = loaded.instance.client_data_mut() {
                out.append(&mut data.commands);
            }
        }
        out
    }
}

/// The `(mod id, client wasm path)` pairs a session activates: every
/// installed id-bearing pack that ships `client_wasm` AND is in the
/// session's enabled set. Pure — the client-side enablement contract,
/// unit-tested against synthetic pack lists (the client twin of
/// `session_wasm_mods` in `modding/mod.rs`).
fn session_client_mods(
    packs: &[crate::assets::Pack],
    enabled: &BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    packs
        .iter()
        .filter_map(|pack| {
            let id = pack.id.clone()?;
            let wasm = pack.client_wasm.clone()?;
            if !enabled.contains(&id) {
                log::info!("client mod '{id}' is not enabled for this session; not loading");
                return None;
            }
            Some((id, wasm))
        })
        .collect()
}

fn dispatch_unit(instance: &mut ModInstance, world: &World, call: &GuestCall, what: &str) {
    match instance.call_guest_client(world, call) {
        None | Some(GuestRet::Unit) => {}
        Some(_) => instance.disable(&format!("returned a non-unit reply to {what}")),
    }
}

/// Whether a physical key is already bound by an engine gameplay control.
/// Derived from [`crate::controls::control_from_key_code`] — the engine's
/// single binding source — so a new engine binding is refused to client mods
/// automatically, with no list to keep in sync.
fn reserved_key(key: &str) -> bool {
    PHYSICAL_KEYS
        .iter()
        .any(|(code, name)| *name == key && crate::controls::control_from_key_code(*code).is_some())
}

fn client_storage_dir(session_key: &str, mod_id: &str) -> PathBuf {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in session_key.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x1_0000_0000_01b3);
    }
    crate::save::base_data_dir()
        .join("client_mod_data")
        .join(format!("{hash:016x}"))
        .join(mod_id)
}

/// The bindable physical keys and their stable ABI names — the one table
/// behind [`physical_key_name`] and [`reserved_key`].
const PHYSICAL_KEYS: &[(winit::keyboard::KeyCode, &str)] = {
    use winit::keyboard::KeyCode;
    &[
        (KeyCode::KeyA, "key_a"),
        (KeyCode::KeyB, "key_b"),
        (KeyCode::KeyC, "key_c"),
        (KeyCode::KeyD, "key_d"),
        (KeyCode::KeyE, "key_e"),
        (KeyCode::KeyF, "key_f"),
        (KeyCode::KeyG, "key_g"),
        (KeyCode::KeyH, "key_h"),
        (KeyCode::KeyI, "key_i"),
        (KeyCode::KeyJ, "key_j"),
        (KeyCode::KeyK, "key_k"),
        (KeyCode::KeyL, "key_l"),
        (KeyCode::KeyM, "key_m"),
        (KeyCode::KeyN, "key_n"),
        (KeyCode::KeyO, "key_o"),
        (KeyCode::KeyP, "key_p"),
        (KeyCode::KeyQ, "key_q"),
        (KeyCode::KeyR, "key_r"),
        (KeyCode::KeyS, "key_s"),
        (KeyCode::KeyT, "key_t"),
        (KeyCode::KeyU, "key_u"),
        (KeyCode::KeyV, "key_v"),
        (KeyCode::KeyW, "key_w"),
        (KeyCode::KeyX, "key_x"),
        (KeyCode::KeyY, "key_y"),
        (KeyCode::KeyZ, "key_z"),
        (KeyCode::Digit0, "digit_0"),
        (KeyCode::Digit1, "digit_1"),
        (KeyCode::Digit2, "digit_2"),
        (KeyCode::Digit3, "digit_3"),
        (KeyCode::Digit4, "digit_4"),
        (KeyCode::Digit5, "digit_5"),
        (KeyCode::Digit6, "digit_6"),
        (KeyCode::Digit7, "digit_7"),
        (KeyCode::Digit8, "digit_8"),
        (KeyCode::Digit9, "digit_9"),
    ]
};

pub(crate) fn physical_key_name(code: winit::keyboard::KeyCode) -> Option<&'static str> {
    PHYSICAL_KEYS
        .iter()
        .find(|(bindable, _)| *bindable == code)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Client mods activate ONLY for packs in the session's enabled set —
    /// on a remote join that is the server's handshake-reported mod list,
    /// so a locally installed client mod the server does not run (e.g. the
    /// minimap against a server without it) stays inactive.
    #[test]
    fn unlisted_packs_contribute_no_client_instance() {
        let pack = |name: &str, id: Option<&str>, client_wasm: Option<&str>| crate::assets::Pack {
            dir: PathBuf::from(format!("/fixture/{name}")),
            name: name.to_owned(),
            id: id.map(str::to_owned),
            version: None,
            description: String::new(),
            summary: None,
            icon: None,
            wasm: None,
            client_wasm: client_wasm.map(PathBuf::from),
        };
        let packs = [
            pack(
                "minimap",
                Some("minimap"),
                Some("/fixture/minimap/client.wasm"),
            ),
            pack("radar", Some("radar"), Some("/fixture/radar/client.wasm")),
            pack("content_only", None, None),
        ];

        let server_reported: BTreeSet<String> = ["radar".to_owned()].into();
        let ids: Vec<String> = session_client_mods(&packs, &server_reported)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["radar"], "only enabled packs activate");

        assert!(
            session_client_mods(&packs, &BTreeSet::new()).is_empty(),
            "a server reporting no mods enables no client mods"
        );
        let all: BTreeSet<String> = ["minimap".to_owned(), "radar".to_owned()].into();
        assert_eq!(session_client_mods(&packs, &all).len(), 2);
    }

    /// The reservation rule is DERIVED from the engine's binding table:
    /// engine-bound keys are refused to client mods, unbound keys are free.
    #[test]
    fn engine_bound_keys_are_reserved_for_client_mods() {
        assert!(reserved_key("key_w"));
        assert!(reserved_key("digit_1"));
        assert!(!reserved_key("key_m"));
        assert!(!reserved_key("key_n"));
    }
}
