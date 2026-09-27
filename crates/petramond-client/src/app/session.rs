//! Everything the App holds for exactly one game session, in one value: the
//! game itself plus the app-side state that only means something while that
//! game runs — the HUD notice, the creative menu's and schematic library's
//! forms, chat, the Open to LAN status, the world-sound scheduling, the
//! HUD/hand effects and the presentation scratch (ambient drives included).
//!
//! The lifecycle is construction and drop: a session starts as
//! [`Session::new`] and ends as [`Session::end`] (or a plain drop), so nothing
//! here can outlive its world and there is no reset list to keep in step with
//! the fields.

use super::chat::ChatUi;
use super::client_audio::SessionSounds;
use super::creative::CreativeMenu;
use super::hotbar_notice::HotbarNotice;
use super::hud_fx::HudFx;
use super::schematic_library::LibraryForm;
use crate::game::presentation::GamePresentationScratch;
use crate::game::section_cache::SectionCache;
use crate::game::Game;
use petramond_render::Scene;

pub(super) struct Session {
    pub(super) game: Game,
    pub(super) hotbar_notice: HotbarNotice,
    pub(super) creative_menu: CreativeMenu,
    pub(super) library_form: LibraryForm,
    /// The port the running HOST session is open to LAN on (`None` = not
    /// open). Drives the pause menu's Open to LAN button/label.
    pub(super) lan_port: Option<u16>,
    /// The last Open to LAN failure, shown inline on the pause menu; cleared
    /// when the pause screen closes.
    pub(super) lan_error: Option<String>,
    /// This session's chat history, draft and scroll.
    pub(super) chat: ChatUi,
    /// World-sound cues, cadences and the client-local handle pool.
    pub(super) sounds: SessionSounds,
    /// Short-lived HUD/hand presentation: hurt shake, sleep-overlay hand,
    /// heart wiggle, and the local rigs' graph events awaiting the next draw.
    pub(super) hud_fx: HudFx,
    /// Reusable builder for neutral per-frame presentation data read from the
    /// game; its ambient (precipitation) drives are this session's.
    pub(super) presentation: GamePresentationScratch,
    /// Render-side translation of neutral per-frame presentation data into
    /// the renderer's wire structs.
    pub(super) scene: Scene,
    _item_bakes: ItemBakeScope,
}

impl Session {
    /// Wrap a freshly-built game. The game's client mods have already baked
    /// their item geometry, so this must not run while a previous session is
    /// still alive (its drop would flush the new bakes).
    pub(super) fn new(game: Game) -> Self {
        Self {
            game,
            hotbar_notice: HotbarNotice::default(),
            creative_menu: CreativeMenu::default(),
            library_form: LibraryForm::default(),
            lan_port: None,
            lan_error: None,
            chat: ChatUi::default(),
            sounds: SessionSounds::default(),
            hud_fx: HudFx::default(),
            presentation: GamePresentationScratch::new(),
            scene: Scene::new(),
            _item_bakes: ItemBakeScope,
        }
    }

    /// End the session: harvest its section cache — a reconnect's Join
    /// manifest claims it, re-promoting cached terrain instead of
    /// re-streaming it — then shut the game's server link down (a host joins
    /// its server thread, which saves; a remote connection says farewell).
    /// Everything else drops with the session.
    pub(super) fn end(self) -> SectionCache {
        let Session { mut game, .. } = self;
        let cache = game.take_section_cache();
        game.shutdown();
        cache
    }
}

/// Baked custom-shape item geometry is a process-wide cache (the renderer's
/// item-cube path reads it) keyed by session-local block ids. The session owns
/// this scope, so the cache is flushed exactly when the session that baked it
/// ends and the next world's mods rebake instead of inheriting stale shapes.
struct ItemBakeScope;

impl Drop for ItemBakeScope {
    fn drop(&mut self) {
        petramond_world::block::item_shape_bake::clear();
    }
}
