//! Host-side state a client instance publishes: images, UI state, retained
//! canvas scenes, overlay/key registrations, and queued shell commands.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::player::RigId;

/// How many consecutive partial updates an image remembers; a consumer whose
/// held revision fell out of the window uploads the whole texture.
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
    /// Put the caret in one of the owner's open document's text inputs.
    FocusInput {
        owner: String,
        id: String,
        item: Option<u32>,
    },
    /// Open the engine's pause menu over the owner's open UI.
    OpenPause {
        owner: String,
    },
}

/// One retained scene: its elements and pan, and a revision that moves on
/// every change so a consumer converts it only when it changed.
#[derive(Clone, Default)]
pub struct ClientCanvasSceneData {
    pub elements: Arc<Vec<mod_api::ClientCanvasElement>>,
    pub offset: [f32; 2],
    pub revision: u64,
}

/// The animator keys a client mod owns locally: `(rig, id)` sets.
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
    /// Part of the HUD (hidden with it), or a tool's own status (not).
    pub hud: bool,
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
    pub mods: mod_api::ClientKeyMods,
    pub contexts: mod_api::ClientKeyContexts,
    pub action_id: u32,
}

/// Where a client instance's two storage buckets live
/// ([`mod_api::ClientStorageScope`]).
pub(in crate::modding) struct ClientBuckets {
    /// The presented session's bucket; `None` = there is no world (the shell).
    pub world: Option<PathBuf>,
    pub pack: PathBuf,
}

impl ClientBuckets {
    /// Both buckets under one scratch directory — a test's, never the
    /// player's data directory.
    #[cfg(test)]
    pub(in crate::modding) fn under(dir: PathBuf) -> Self {
        Self {
            world: Some(dir.join("world")),
            pack: dir.join("pack"),
        }
    }
}

pub(in crate::modding) struct ClientStoreData {
    /// The session's bucket ([`mod_api::ClientStorageScope::World`]); `None`
    /// on the shell, where there is no world to keep anything for.
    pub(super) storage: Option<super::storage::ClientStorage>,
    /// The mod's own bucket ([`mod_api::ClientStorageScope::Pack`]).
    pub(super) pack_storage: super::storage::ClientStorage,
    /// This mod's file tickets, in either bucket.
    pub(super) files: super::files::FileTickets,
    /// Whether this instance runs on the shell with no world right now; the
    /// host moves a launched instance between the shell and a presentation's
    /// world. What the world IS, the presented desk says.
    pub shell: bool,
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
    pub sound_loops: BTreeMap<petramond_world::sound_registry::Sound, f32>,
    /// This mod's post-process mood `[darken, desaturate]` (each `0..=0.5`).
    /// Mods combine by max; eased app-side before it reaches the grade pass.
    pub mood: [f32; 2],
    /// This mod's PREDICTED body claims for the local player — today the
    /// per-hand held poses . The same `BodyClaims` the
    /// server resolves, so a mod that runs its rule on both sides cannot
    /// have the two answers disagree by construction; the client's answer
    /// simply arrives a round trip earlier.
    pub body: crate::player::BodyClaims,
    /// Whether this mod has ever posed each hand (`[main, off]`), latched on
    /// the first NON-empty write.
    ///
    /// A mod that poses a hand owns it locally from then on, because the
    /// moment it RELEASES that pose has to present immediately — falling back
    /// to the replicated answer there would hold the stale pose for a whole
    /// round trip, which is the latency prediction exists to remove. Latching
    /// on a non-empty write (rather than on any write) is what keeps a mod
    /// that publishes "nothing" every frame from claiming hands it never
    /// touches, so another pack's server-side pose still lands there.
    pub poses_hands: [bool; 2],
    /// Which hands this mod has ever DRESSED (`[main, off]`), latched like
    /// [`poses_hands`](Self::poses_hands) and for the same reason.
    pub displays_hands: [bool; 2],
    /// Which rig BONES this mod has ever offset, latched like
    /// [`poses_hands`](Self::poses_hands) and for the same reason: a mod that
    /// bends an arm owns it locally, so straightening it again presents on
    /// this frame rather than a round trip later.
    ///
    /// Per BONE rather than per body, because bone offsets COMPOSE: one pack
    /// predicting a shoulder must leave another pack's replicated head tilt
    /// exactly where it was.
    pub poses_bones: BTreeSet<u16>,
    /// Which animator params and slots this mod has ever claimed, latched
    /// like [`poses_bones`](Self::poses_bones) and for the same reason — per
    /// key, because animator claims compose per key. Events are not latched:
    /// a fire is an edge, so the runtime matches each one to the server's
    /// echo one for one instead.
    pub owns_animator: AnimatorOwnership,
    /// Graph events this mod fired since the tick last drained them.
    pub animator_events: Vec<(RigId, u16)>,
    /// Whether this mod holds the LOCAL player's current use gesture — the
    /// predicted twin of the session's owner, latched by `HoldUse` and cleared
    /// when the button comes up.
    pub holds_use: bool,
    /// This mod's retained VIEW CLAIMS (camera, chrome, perspective, shader
    /// param overrides), folded across mods by [`super::view::fold`].
    pub view: super::view::ViewClaims,
    /// The runtime's frame, clock, tap and media desk, shared the same way.
    pub media: super::media::MediaDesk,
    /// What the app presented; the runtime installs its own.
    pub presented: super::presented::PresentedDesk,
    /// This mod's retained world marks, validated at the call.
    /// The mod's world mark sets, by name.
    pub world_marks: std::collections::BTreeMap<String, Vec<mod_api::ClientWorldMark>>,
    pub commands: Vec<ClientCommand>,
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
        }
    }
}
