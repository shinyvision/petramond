use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::player::RigId;

pub const IMAGE_BLIT_WINDOW: usize = 8;

#[derive(Clone)]
pub struct ClientImageData {
    pub key: String,
    pub width: u16,
    pub height: u16,
    pub rgba: Arc<[u8]>,
    pub revision: u64,
    /// The last `IMAGE_BLIT_WINDOW` partial updates as (revision after the
    /// blit, pixel rect `[x, y, w, h]`), oldest first and CONSECUTIVE up to
    /// `revision`: a consumer holding revision R ≥ `blits[0].0 − 1` needs
    /// only the union of rects with revision > R. Cleared by any whole-image
    /// mutation (re-publish, text draws).
    pub recent_blits: Vec<(u64, [u16; 4])>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientCommand {
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
    FocusInput {
        owner: String,
        id: String,
        item: Option<u32>,
    },
    OpenPause {
        owner: String,
    },
}

#[derive(Clone, Default)]
pub struct ClientCanvasSceneData {
    pub elements: Arc<Vec<mod_api::ClientCanvasElement>>,
    pub offset: [f32; 2],
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimatorOwnership {
    pub params: BTreeSet<(RigId, u16)>,
    pub slots: BTreeSet<(RigId, u16)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientOverlayRegistration {
    pub image_key: String,
    pub anchor: mod_api::ClientOverlayAnchor,
    pub margin: [u16; 2],
    pub display_size: [u16; 2],
    pub hud: bool,
}

#[derive(Clone)]
pub(in crate::modding) struct ClientKeyBinding {
    pub id: String,
    pub label: String,
    pub key: String,
    pub mods: mod_api::ClientKeyMods,
    pub contexts: mod_api::ClientKeyContexts,
    pub action_id: u32,
}

pub(in crate::modding) struct ClientBuckets {
    pub world: Option<PathBuf>,
    pub pack: PathBuf,
}

impl ClientBuckets {
    #[cfg(test)]
    pub(in crate::modding) fn under(dir: PathBuf) -> Self {
        Self {
            world: Some(dir.join("world")),
            pack: dir.join("pack"),
        }
    }
}

pub(in crate::modding) struct ClientStoreData {
    pub(super) storage: Option<super::storage::ClientStorage>,
    pub(super) pack_storage: super::storage::ClientStorage,
    pub(super) files: super::files::FileTickets,
    pub shell: bool,
    pub overlays: Vec<ClientOverlayRegistration>,
    pub key_bindings: Vec<ClientKeyBinding>,
    pub ui_state: Arc<BTreeMap<String, mod_api::GuiValue>>,
    pub images: BTreeMap<String, ClientImageData>,
    pub canvas_scenes: BTreeMap<String, ClientCanvasSceneData>,
    pub ambient_sets: BTreeMap<u8, (f32, [f32; 2])>,
    pub sound_loops: BTreeMap<petramond_world::sound_registry::Sound, f32>,
    pub mood: [f32; 2],
    pub cloth_wind: Option<[f32; 2]>,
    pub body: crate::player::BodyClaims,
    pub poses_hands: [bool; 2],
    pub displays_hands: [bool; 2],
    pub poses_bones: BTreeSet<u16>,
    pub owns_animator: AnimatorOwnership,
    pub animator_events: Vec<(RigId, u16)>,
    pub holds_use: bool,
    pub view: super::view::ViewClaims,
    pub media: super::media::MediaDesk,
    pub presented: super::presented::PresentedDesk,
    pub world_marks: std::collections::BTreeMap<String, Vec<mod_api::ClientWorldMark>>,
    pub commands: Vec<ClientCommand>,
    pub outbound_events: Vec<(String, Vec<u8>)>,
}

impl ClientStoreData {
    pub(in crate::modding) fn new(buckets: ClientBuckets) -> Self {
        let shell = buckets.world.is_none();
        Self {
            storage: buckets.world.map(super::storage::ClientStorage::new),
            pack_storage: super::storage::ClientStorage::new(buckets.pack),
            files: Default::default(),
            shell,
            overlays: Vec::new(),
            key_bindings: Vec::new(),
            ui_state: Arc::new(BTreeMap::new()),
            images: BTreeMap::new(),
            canvas_scenes: BTreeMap::new(),
            ambient_sets: BTreeMap::new(),
            sound_loops: BTreeMap::new(),
            mood: [0.0, 0.0],
            cloth_wind: None,
            body: Default::default(),
            poses_hands: [false; 2],
            displays_hands: [false; 2],
            poses_bones: BTreeSet::new(),
            owns_animator: AnimatorOwnership::default(),
            animator_events: Vec::new(),
            holds_use: false,
            view: Default::default(),
            media: Default::default(),
            presented: Default::default(),
            world_marks: Default::default(),
            commands: Vec::new(),
            outbound_events: Vec::new(),
        }
    }
}
