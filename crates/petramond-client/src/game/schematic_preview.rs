//! A schematic held up for placement: it follows the crosshair on its
//! rotated footprint's pivot, turns in quarter steps, shifts vertically, and
//! commits with the place click — as a paste, or as the answer to a
//! positioning the server opened.

use super::{Game, GameInput};
use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond::player::{Player, RayFilter, RaycastHit};
use petramond::schematic::share::SchematicRequest;
use petramond::schematic::store::Digest;
use petramond::schematic::{Scene, Schematic, PLACEMENT_REACH};
use std::sync::Arc;

/// The positioning a preview answers instead of pasting.
struct Positioning {
    tag: String,
    digest: Digest,
    /// Where the preview rests while the crosshair holds no target.
    resting_origin: Option<[i32; 3]>,
}

struct Held {
    schematic: Arc<Schematic>,
    positioning: Option<Positioning>,
    origin: Option<[i32; 3]>,
    turns: u8,
    vertical_offset: i32,
}

#[derive(Default)]
pub struct SchematicPreview {
    held: Option<Held>,
    scene: Option<Arc<Scene>>,
    scene_key: Option<(Arc<Schematic>, u8)>,
}

impl SchematicPreview {
    pub fn is_up(&self) -> bool {
        self.held.is_some()
    }

    pub fn schematic(&self) -> Option<&Arc<Schematic>> {
        self.held.as_ref().map(|held| &held.schematic)
    }

    /// The tag of the positioning this preview answers, if it is one.
    pub fn positioning_tag(&self) -> Option<&str> {
        let positioning = self.held.as_ref()?.positioning.as_ref()?;
        Some(&positioning.tag)
    }

    pub fn origin(&self) -> Option<[i32; 3]> {
        self.held.as_ref()?.origin
    }

    pub fn turns(&self) -> u8 {
        self.held.as_ref().map_or(0, |held| held.turns)
    }

    pub fn vertical_offset(&self) -> i32 {
        self.held.as_ref().map_or(0, |held| held.vertical_offset)
    }

    /// The meshable scene of the held schematic at its current turn.
    pub fn scene(&self) -> Option<&Arc<Scene>> {
        self.scene.as_ref()
    }

    pub(crate) fn begin_paste(&mut self, schematic: Arc<Schematic>) {
        self.held = Some(Held {
            schematic,
            positioning: None,
            origin: None,
            turns: 0,
            vertical_offset: 0,
        });
    }

    pub(super) fn begin_positioning(
        &mut self,
        schematic: Arc<Schematic>,
        tag: String,
        digest: Digest,
        resting_origin: Option<[i32; 3]>,
        turns: u8,
    ) {
        self.held = Some(Held {
            schematic,
            positioning: Some(Positioning {
                tag,
                digest,
                resting_origin,
            }),
            origin: None,
            turns: turns % 4,
            vertical_offset: 0,
        });
    }

    /// Whether there was a preview to take down.
    pub(super) fn cancel(&mut self) -> bool {
        std::mem::take(self).held.is_some()
    }

    fn rotate(&mut self) {
        if let Some(held) = &mut self.held {
            held.turns = (held.turns + 1) % 4;
        }
    }

    fn raise(&mut self, blocks: i32) {
        if let Some(held) = &mut self.held {
            held.vertical_offset = held.vertical_offset.saturating_add(blocks);
        }
    }

    /// Rest the preview against the block face under the crosshair.
    fn aim(&mut self, hit: Option<RaycastHit>) {
        let Some(held) = &mut self.held else {
            return;
        };
        let resting = held.positioning.as_ref().and_then(|p| p.resting_origin);
        held.origin = hit
            .and_then(|h| {
                let mut origin = h.block + h.normal - held.schematic.placement_pivot(held.turns);
                origin.y = origin.y.checked_add(held.vertical_offset)?;
                Some(origin.to_array())
            })
            .or(resting);
    }
}

impl Game {
    /// A preview is up that this mode may commit: a paste in creative, or a
    /// positioning in any mode.
    pub fn schematic_preview_active(&self) -> bool {
        self.schematic_preview
            .held
            .as_ref()
            .is_some_and(|held| self.player.is_creative() || held.positioning.is_some())
    }

    pub fn rotate_schematic_preview(&mut self) -> bool {
        let active = self.schematic_preview_active();
        if active {
            self.schematic_preview.rotate();
        }
        active
    }

    pub fn raise_schematic_preview(&mut self, blocks: i32) -> bool {
        let active = self.schematic_preview_active();
        if active {
            self.schematic_preview.raise(blocks);
        }
        active
    }

    pub fn schematic_scene(
        &mut self,
        schematic: &Schematic,
        turns: u8,
    ) -> Result<Arc<Scene>, String> {
        Scene::prepare(schematic, turns, |world| {
            self.client_mods.bake_custom_shapes(world)
        })
        .map(Arc::new)
    }

    /// One frame of a preview that is up: keep its scene current, follow the
    /// crosshair, and commit on the place click.
    pub(super) fn schematic_preview_input(&mut self, input: &GameInput) {
        self.prepare_preview_scene();
        let hit = Player::raycast_filtered(
            self.cam.pos,
            self.cam.forward(),
            PLACEMENT_REACH,
            RayFilter::Selectable,
            &self.replica,
        )
        .map(|(h, _)| h);
        self.schematic_preview.aim(hit);
        if input.gameplay_enabled && input.place_clicked {
            self.commit_schematic_preview();
        }
    }

    fn commit_schematic_preview(&mut self) {
        let Some(held) = self.schematic_preview.held.as_mut() else {
            return;
        };
        let Some(origin) = held.origin else {
            return;
        };
        let turns = held.turns;
        if let Some(Positioning { tag, digest, .. }) = held.positioning.take() {
            self.outbox
                .push(ClientToServer::Action(PlayerAction::Schematic(
                    SchematicRequest::Positioned {
                        tag,
                        digest,
                        origin,
                        turns,
                    },
                )));
            self.cancel_world_tools();
        } else {
            let schematic = held.schematic.clone();
            self.paste_schematic(schematic, origin, turns);
        }
    }

    fn prepare_preview_scene(&mut self) {
        let Some(held) = &self.schematic_preview.held else {
            self.schematic_preview.scene = None;
            self.schematic_preview.scene_key = None;
            return;
        };
        let (schematic, turns) = (held.schematic.clone(), held.turns);
        let current = self
            .schematic_preview
            .scene_key
            .as_ref()
            .is_some_and(|(s, t)| *t == turns && Arc::ptr_eq(s, &schematic));
        if current {
            return;
        }
        self.schematic_preview.scene_key = Some((schematic.clone(), turns));
        match self.schematic_scene(&schematic, turns) {
            Ok(scene) => self.schematic_preview.scene = Some(scene),
            Err(error) => {
                self.schematic_preview.scene = None;
                self.notice = error;
            }
        }
    }
}
