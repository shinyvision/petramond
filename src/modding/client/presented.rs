use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use petramond_world::tile::Tile;

pub struct Presented {
    pub context: mod_api::ClientContext,
    pub view: Option<mod_api::ClientViewStateData>,
    pub entities: Vec<mod_api::ClientEntityData>,
    pub key_labels: BTreeMap<String, String>,
    pub local_player: Option<mod_api::PlayerId>,
    pub screen: Option<String>,
    pub text_inputs: Vec<(String, Option<u32>)>,
    pub menu: Option<mod_api::ClientMenuData>,
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

/// Reads one tile's pixels out of the atlas that is current when it is called.
pub type TilePixels = fn(Tile) -> Option<Vec<u8>>;

static TILE_PIXELS: OnceLock<TilePixels> = OnceLock::new();

pub fn publish_tile_pixels(source: TilePixels) {
    let _ = TILE_PIXELS.set(source);
}

pub fn tile_pixels(name: &str) -> Option<Vec<u8>> {
    let source = TILE_PIXELS.get()?;
    source(Tile::from_name(name)?)
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
            menu: None,
            frame_limits: DEVICE_FRAME_LIMITS.get().copied(),
            capture: Default::default(),
            presentation: Default::default(),
        }
    }
}

pub fn menu_data(
    kind_key: &str,
    anchor: Option<crate::menu::MenuAnchor>,
    slots: &[Option<petramond_world::item::ItemStack>],
) -> mod_api::ClientMenuData {
    mod_api::ClientMenuData {
        kind_key: kind_key.to_owned(),
        at: anchor.map(crate::modding::convert::container_address),
        slots: slots
            .iter()
            .map(|slot| slot.map(crate::modding::host::guards::item_stack_data))
            .collect(),
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
