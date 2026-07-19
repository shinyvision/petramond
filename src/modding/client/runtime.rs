//! Presentation-only client WASM instances.
//!
//! A pack opts in with `client_wasm`; this runtime is separate from the
//! deterministic server/worldgen instances. It can sample final cells from
//! the client's replica, publish document state/images, receive registered
//! key, GUI, and canvas events, and persist namespaced blobs in a host-owned sandbox.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
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

/// One mod-registered remappable key action, resolved for this session.
pub(crate) struct ModKeyAction {
    /// Namespaced identity (`mod_id:action`): the remap-persistence key, the
    /// dispatch handle, and the controls-screen row id.
    pub full_id: String,
    pub label: String,
    /// Controls-screen category: the owning pack's display name.
    pub category: String,
    /// The registered DEFAULT key (the player may remap it away).
    pub default_code: winit::keyboard::KeyCode,
    mod_index: usize,
    action_id: u32,
}

pub(crate) struct ClientModRuntime {
    mods: Vec<ClientMod>,
    actions: Vec<ModKeyAction>,
    overlays: Vec<super::state::ClientOverlayRegistration>,
    /// Currently-down action `full_id`s — the edge filter for `ClientKey`
    /// dispatch, whatever input the player bound.
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
        let session = session_client_mods(crate::assets::packs(), enabled);
        crate::modding::host::module_cache::prewarm(session.iter().map(|(_, path)| path.clone()));
        for (id, path) in session {
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

        let mut actions = Vec::new();
        let mut overlays = Vec::new();
        for (index, loaded) in mods.iter().enumerate() {
            let Some(data) = loaded.instance.client_data() else {
                continue;
            };
            // Overlay image keys are namespace-guarded at registration (and
            // per-mod duplicates rejected there), so keys are already unique
            // across mods.
            overlays.extend(data.overlays.iter().cloned());
            // Category = the pack's display name; the id keys the category
            // when a pack somehow has no display row.
            let category = crate::assets::packs()
                .iter()
                .find(|p| p.id.as_deref() == Some(loaded.id.as_str()))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| loaded.id.clone());
            for binding in &data.key_bindings {
                // The DEFAULT may not shadow an engine default — the player
                // could no longer tell who owns the key out of the box.
                // (Remaps are the player's own choice and are not policed.)
                if reserved_key(&binding.key) {
                    log::error!(
                        "client mod '{}': default key '{}' conflicts with an engine binding; \
                         action '{}' ignored",
                        loaded.id,
                        binding.key,
                        binding.id
                    );
                    continue;
                }
                let Some(default_code) = key_code_for_name(&binding.key) else {
                    log::error!(
                        "client mod '{}': unknown default key '{}'; action '{}' ignored",
                        loaded.id,
                        binding.key,
                        binding.id
                    );
                    continue;
                };
                actions.push(ModKeyAction {
                    full_id: format!("{}:{}", loaded.id, binding.id),
                    label: binding.label.clone(),
                    category: category.clone(),
                    default_code,
                    mod_index: index,
                    action_id: binding.action_id,
                });
            }
        }
        Self {
            mods,
            actions,
            overlays,
            pressed: HashSet::new(),
        }
    }

    /// The session's mod-registered remappable actions, for the app's action
    /// table and the controls screen.
    pub(crate) fn key_actions(&self) -> &[ModKeyAction] {
        &self.actions
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

    /// Dispatch one bound-action edge to its owning mod, by the action's
    /// namespaced `full_id`. Returns whether a live mod owns the action.
    pub(crate) fn action(&mut self, world: &World, full_id: &str, pressed: bool) -> bool {
        let Some((index, action_id)) = self
            .actions
            .iter()
            .find(|a| a.full_id == full_id)
            .map(|a| (a.mod_index, a.action_id))
        else {
            return false;
        };
        if self.mods[index].instance.disabled() {
            self.pressed.remove(full_id);
            return false;
        }
        let was_pressed = self.pressed.contains(full_id);
        if was_pressed == pressed {
            return true;
        }
        if pressed {
            self.pressed.insert(full_id.to_owned());
        } else {
            self.pressed.remove(full_id);
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

    pub(crate) fn canvas_scroll(
        &mut self,
        world: &World,
        canvas_key: &str,
        x: f32,
        y: f32,
        delta: f32,
    ) {
        let call = GuestCall::ClientCanvasScroll {
            canvas_key: canvas_key.to_owned(),
            x,
            y,
            delta,
        };
        let Some(loaded) = self.owner_mod_mut(canvas_key) else {
            return;
        };
        dispatch_unit(&mut loaded.instance, world, &call, "client canvas scroll");
    }

    pub(crate) fn release_all_keys(&mut self, world: &World) {
        let pressed: Vec<_> = self.pressed.drain().collect();
        for full_id in pressed {
            let Some((index, action_id)) = self
                .actions
                .iter()
                .find(|a| a.full_id == full_id)
                .map(|a| (a.mod_index, a.action_id))
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

    /// Push every mod's current ambient-volume targets into `drives`
    /// (per-(mod, bundle) keyed — two mods driving one bundle stay
    /// independent). The drives ease and derive; this only syncs targets.
    pub(crate) fn sync_ambient_targets(&self, drives: &mut crate::game::ambient::AmbientDrives) {
        for m in &self.mods {
            // A disabled (trapped/watchdogged) mod must not freeze its last
            // weather on for the rest of the session: zero its targets so
            // the drives ease out and retire (mirrors `take_commands`).
            let disabled = m.instance.disabled();
            let Some(data) = m.instance.client_data() else {
                continue;
            };
            for (&bundle, &(intensity, wind)) in &data.ambient_sets {
                let intensity = if disabled { 0.0 } else { intensity };
                drives.set(&m.id, bundle, intensity, wind);
            }
        }
    }

    /// Every mod's looping-sound gains: `(sound, gain)`, mods in load order.
    /// The audio side keys its loop table on the resolved sound, so two mods
    /// driving one sound key resolve last-writer-wins there. Disabled mods
    /// contribute nothing — their loops sweep to silence.
    pub(crate) fn sound_loops(&self, out: &mut Vec<(crate::audio::Sound, f32)>) {
        out.clear();
        for m in &self.mods {
            if m.instance.disabled() {
                continue;
            }
            let Some(data) = m.instance.client_data() else {
                continue;
            };
            out.extend(data.sound_loops.iter().map(|(&s, &g)| (s, g)));
        }
    }

    /// The combined post-process mood: component-wise MAX over enabled mods
    /// (disabled mods contribute nothing — their mood dies with them).
    pub(crate) fn mood(&self) -> [f32; 2] {
        let mut mood = [0.0f32, 0.0];
        for m in &self.mods {
            if m.instance.disabled() {
                continue;
            }
            if let Some(data) = m.instance.client_data() {
                mood[0] = mood[0].max(data.mood[0]);
                mood[1] = mood[1].max(data.mood[1]);
            }
        }
        mood
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

/// Whether a physical key is already bound by an engine gameplay control:
/// the fixed (non-remappable) table plus every DEFAULT action binding. The
/// player's live remaps deliberately don't move this set — a mod key that was
/// valid at pack load must not turn invalid because the player rebound Sneak.
fn reserved_key(key: &str) -> bool {
    let defaults = crate::controls::BindingSet::default();
    let default_bound = |code: winit::keyboard::KeyCode| {
        crate::controls::BindableAction::ALL
            .iter()
            .any(|a| defaults.binding(*a).input == crate::controls::BoundInput::Key(code))
    };
    PHYSICAL_KEYS.iter().any(|(code, name)| {
        *name == key
            && (crate::controls::fixed_control_from_key_code(*code).is_some()
                || default_bound(*code))
    })
}

/// The client-storage identity of a LOCAL world: its save-DIRECTORY name,
/// never the display name. A world's directory never changes after creation
/// (renames rewrite only `world.json`), so personal mod data — minimap
/// exploration, waypoints — follows a renamed world with zero migration.
pub(crate) fn local_session_key(world_dir_name: &str) -> String {
    format!("local:{world_dir_name}")
}

/// The client-storage identity of a remote server (its address string).
pub(crate) fn remote_session_key(server_identity: &str) -> String {
    format!("remote:{server_identity}")
}

/// The ONE bucket holding every client mod's sandboxed storage for a session
/// identity: `<base>/client_mod_data/<fnv1a64(session_key)>/<mod_id>/...` —
/// this is the `<mod_id>`s' parent, the unit world deletion removes.
fn session_storage_bucket(base: &Path, session_key: &str) -> PathBuf {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in session_key.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x1_0000_0000_01b3);
    }
    base.join("client_mod_data").join(format!("{hash:016x}"))
}

fn client_storage_dir(session_key: &str, mod_id: &str) -> PathBuf {
    session_storage_bucket(&crate::save::base_data_dir(), session_key).join(mod_id)
}

#[cfg(test)]
pub(crate) fn client_storage_dir_for_test(session_key: &str, mod_id: &str) -> PathBuf {
    client_storage_dir(session_key, mod_id)
}

/// Test seeding: write entries into a mod's session storage bucket through
/// the ordered worker, flushed before return — perf harnesses fabricate a
/// large explored world without driving real exploration.
#[cfg(test)]
pub(crate) fn seed_client_storage_for_test(
    session_key: &str,
    mod_id: &str,
    mut entries: Vec<(String, Vec<u8>)>,
) {
    let mut storage = super::storage::ClientStorage::new(client_storage_dir(session_key, mod_id));
    while !entries.is_empty() {
        // Stay under the per-batch byte cap whatever the value sizes.
        let mut take = 0usize;
        let mut bytes = 0usize;
        while take < entries.len() && take < 512 && bytes < 8 << 20 {
            bytes += entries[take].0.len() + entries[take].1.len();
            take += 1;
        }
        let rest = entries.split_off(take);
        storage
            .set_many(entries)
            .expect("seed client storage batch");
        entries = rest;
    }
    // Drop flushes the worker, so files exist when the test proceeds.
}

/// Delete every client mod's sandboxed storage for a LOCAL world — the
/// world-deletion hook. Exploration maps and waypoints live OUTSIDE the save
/// (personal data, keyed on the world's directory name); without this, a
/// future world reusing the deleted world's directory name would inherit
/// them — explored terrain from a dead seed on a supposed-to-be-black map.
/// Safe against in-flight writes: the storage worker drains synchronously on
/// session drop, and deletion is only reachable from the world-select menu.
pub(crate) fn delete_local_world_storage(world_dir_name: &str) -> std::io::Result<()> {
    delete_local_world_storage_at(&crate::save::base_data_dir(), world_dir_name)
}

fn delete_local_world_storage_at(base: &Path, world_dir_name: &str) -> std::io::Result<()> {
    let bucket = session_storage_bucket(base, &local_session_key(world_dir_name));
    match std::fs::remove_dir_all(bucket) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The bindable physical keys and their stable ABI names — the one table
/// behind [`key_code_for_name`] and [`reserved_key`].
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

/// The `KeyCode` behind a registered default-key name (`"key_m"` → `KeyM`).
pub(crate) fn key_code_for_name(name: &str) -> Option<winit::keyboard::KeyCode> {
    PHYSICAL_KEYS
        .iter()
        .find(|(_, bindable)| *bindable == name)
        .map(|(code, _)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deleting a world's client-mod storage removes that world's WHOLE
    /// bucket — every mod's data, nothing of any other identity — and a
    /// world that never stored anything deletes cleanly. Guards the
    /// world-deletion hook against key-derivation drift: a renamed hash or
    /// session-key format that no longer matches what the runtime writes
    /// would silently orphan (or worse, miss) the data again.
    #[test]
    fn world_deletion_removes_exactly_its_own_storage_bucket() {
        let base = std::env::temp_dir().join(format!(
            "petramond-client-storage-delete-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let dir_for = |session_key: &str, mod_id: &str| {
            session_storage_bucket(&base, session_key).join(mod_id)
        };
        for (key, mod_id) in [
            (local_session_key("doomed"), "minimap"),
            (local_session_key("doomed"), "othermod"),
            (local_session_key("kept"), "minimap"),
            (remote_session_key("play.example.org"), "minimap"),
        ] {
            let dir = dir_for(&key, mod_id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("blob"), b"tile").unwrap();
        }

        delete_local_world_storage_at(&base, "doomed").unwrap();
        assert!(
            !session_storage_bucket(&base, &local_session_key("doomed")).exists(),
            "the deleted world's bucket goes whole — every mod's data"
        );
        assert!(
            dir_for(&local_session_key("kept"), "minimap")
                .join("blob")
                .exists(),
            "another world's bucket is untouched"
        );
        assert!(
            dir_for(&remote_session_key("play.example.org"), "minimap")
                .join("blob")
                .exists(),
            "server buckets are untouched"
        );
        // Idempotent: a world with no client-mod data deletes cleanly.
        delete_local_world_storage_at(&base, "doomed").unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

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
