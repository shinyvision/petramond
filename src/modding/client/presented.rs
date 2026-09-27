use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

pub struct Presented {
    pub context: mod_api::ClientContext,
    pub view: Option<mod_api::ClientViewStateData>,
    pub entities: Vec<mod_api::ClientEntityData>,
    pub key_labels: BTreeMap<String, String>,
    pub local_player: Option<mod_api::PlayerId>,
    pub screen: Option<String>,
    pub text_inputs: Vec<(String, Option<u32>)>,
    pub frame_limits: Option<FrameLimits>,
    pub capture: crate::capture::desk::CaptureDesk,
    pub presentation: super::present::PresentationDesk,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FrameLimits {
    pub max_side: u32,
    pub max_bytes: u64,
}

static DEVICE_FRAME_LIMITS: OnceLock<FrameLimits> = OnceLock::new();

impl FrameLimits {
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
