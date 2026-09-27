//! What the app PRESENTED, as client host calls answer it. The app writes it
//! once per frame (and whenever the player's bindings change); every host
//! call that answers "what is on screen" reads it here, in any dispatch —
//! a frame, a key, a document event — so an answer never depends on which
//! callback asked. One desk per runtime, shared by every mod in it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

pub struct Presented {
    /// What session the mods run beside (a shell instance answers `Shell`
    /// whatever this says).
    pub context: mod_api::ClientContext,
    /// The frame that last presented, as `ClientViewState` answers it.
    pub view: Option<mod_api::ClientViewStateData>,
    /// Every replicated player and mob that frame drew.
    pub entities: Vec<mod_api::ClientEntityData>,
    /// Every registered key action's CURRENT binding label, by `mod_id:id`.
    pub key_labels: BTreeMap<String, String>,
    /// The local player's id in the presented session.
    pub local_player: Option<mod_api::PlayerId>,
    /// The client document kind or canvas key on screen, if a mod's is.
    pub screen: Option<String>,
    /// That document's enabled text inputs as `(id, list item)`.
    pub text_inputs: Vec<(String, Option<u32>)>,
    /// The rendering device's limits a captured frame meets; `None` = this
    /// process has no rendering device.
    pub frame_limits: Option<FrameLimits>,
    /// The presented world's moment and the events logs.
    pub capture: crate::capture::desk::CaptureDesk,
    /// The presentation the mods drive, and where it stands.
    pub presentation: super::present::PresentationDesk,
}

/// The rendering device's own limits on one frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FrameLimits {
    /// The largest 2D texture side.
    pub max_side: u32,
    /// The largest buffer: a frame's readback.
    pub max_bytes: u64,
}

static DEVICE_FRAME_LIMITS: OnceLock<FrameLimits> = OnceLock::new();

impl FrameLimits {
    /// Record the process's rendering device, once it exists, so every
    /// runtime answers its limits from its first call (a mod's `mod_init`).
    pub fn publish(self) {
        let _ = DEVICE_FRAME_LIMITS.set(self);
    }
}

impl Default for Presented {
    fn default() -> Self {
        Presented {
            context: mod_api::ClientContext::Shell,
            view: None,
            entities: Vec::new(),
            local_player: None,
            key_labels: BTreeMap::new(),
            screen: None,
            text_inputs: Vec::new(),
            frame_limits: DEVICE_FRAME_LIMITS.get().copied(),
            capture: Default::default(),
            presentation: Default::default(),
        }
    }
}

#[derive(Clone, Default)]
pub struct PresentedDesk(Arc<Mutex<Presented>>);

impl PresentedDesk {
    pub fn new(context: mod_api::ClientContext) -> Self {
        let desk = PresentedDesk::default();
        desk.lock().context = context;
        desk
    }

    pub fn lock(&self) -> MutexGuard<'_, Presented> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
