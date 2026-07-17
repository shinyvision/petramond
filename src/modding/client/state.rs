//! Host-side state a client instance publishes: images, UI state, retained
//! canvas scenes, overlay/key registrations, and queued shell commands.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// How many consecutive partial updates an image remembers; a consumer whose
/// held revision fell out of the window uploads the whole texture.
pub(crate) const IMAGE_BLIT_WINDOW: usize = 8;

#[derive(Clone)]
pub(crate) struct ClientImageData {
    pub key: String,
    pub width: u16,
    pub height: u16,
    pub rgba: Arc<[u8]>,
    pub revision: u64,
    /// The last [`IMAGE_BLIT_WINDOW`] partial updates as (revision after the
    /// blit, pixel rect `[x, y, w, h]`), oldest first and CONSECUTIVE up to
    /// `revision`: a consumer holding revision R ≥ `blits[0].0 − 1` needs
    /// only the union of rects with revision > R. Cleared by any whole-image
    /// mutation (re-publish, text draws).
    pub recent_blits: Vec<(u64, [u16; 4])>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClientCommand {
    OpenGui {
        owner: String,
        kind: String,
    },
    CloseGui {
        owner: String,
    },
    OpenCanvas {
        owner: String,
        canvas_key: String,
        size: [u16; 2],
    },
    CloseCanvas {
        owner: String,
    },
}

#[derive(Clone, Default)]
pub(crate) struct ClientCanvasSceneData {
    pub elements: Vec<mod_api::ClientCanvasElement>,
    pub offset: [f32; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientOverlayRegistration {
    pub image_key: String,
    pub anchor: mod_api::ClientOverlayAnchor,
    pub margin: [u16; 2],
    pub display_size: [u16; 2],
}

/// One registered remappable key action (`ClientRegisterKey`): the bare id
/// (the player's remap persists as `mod_id:id`), the controls-screen label,
/// the DEFAULT physical key name, and the opaque action id the mod's
/// `client_key` handler matches on.
#[derive(Clone)]
pub(in crate::modding) struct ClientKeyBinding {
    pub id: String,
    pub label: String,
    pub key: String,
    pub action_id: u32,
}

pub(in crate::modding) struct ClientStoreData {
    pub(super) storage: super::storage::ClientStorage,
    pub overlays: Vec<ClientOverlayRegistration>,
    pub key_bindings: Vec<ClientKeyBinding>,
    pub ui_state: Arc<BTreeMap<String, mod_api::GuiValue>>,
    pub images: BTreeMap<String, ClientImageData>,
    pub canvas_scenes: BTreeMap<String, ClientCanvasSceneData>,
    /// Ambient particle-volume targets this mod drives: bundle id →
    /// (intensity, wind). Read by the game's presentation each frame; the
    /// engine eases intensity changes there. A zero-intensity entry stays (it
    /// is the ease-out request); the map is bounded by the bundle catalog.
    pub ambient_sets: BTreeMap<u8, (f32, [f32; 2])>,
    /// Looping-sound gains this mod drives: resolved sound → gain. Read by
    /// the app each frame; gain changes are eased audio-side. Zero-gain
    /// entries stay (the ease-to-silence request); bounded by sounds.json.
    pub sound_loops: BTreeMap<crate::audio::Sound, f32>,
    /// This mod's post-process mood `[darken, desaturate]` (each `0..=0.5`).
    /// Mods combine by max; eased app-side before it reaches the grade pass.
    pub mood: [f32; 2],
    pub commands: Vec<ClientCommand>,
    pub(super) next_image_revision: u64,
}

impl ClientStoreData {
    pub(in crate::modding) fn new(storage_dir: PathBuf) -> Self {
        Self {
            storage: super::storage::ClientStorage::new(storage_dir),
            overlays: Vec::new(),
            key_bindings: Vec::new(),
            ui_state: Arc::new(BTreeMap::new()),
            images: BTreeMap::new(),
            canvas_scenes: BTreeMap::new(),
            ambient_sets: BTreeMap::new(),
            sound_loops: BTreeMap::new(),
            mood: [0.0, 0.0],
            commands: Vec::new(),
            next_image_revision: 1,
        }
    }
}
