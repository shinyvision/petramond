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
    pub(super) lan_port: Option<u16>,
    pub(super) lan_error: Option<String>,
    pub(super) chat: ChatUi,
    pub(super) sounds: SessionSounds,
    pub(super) hud_fx: HudFx,
    pub(super) presentation: GamePresentationScratch,
    pub(super) scene: Scene,
    _item_bakes: ItemBakeScope,
}

impl Session {
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

    pub(super) fn end(self) -> SectionCache {
        let Session { mut game, .. } = self;
        let cache = game.take_section_cache();
        game.shutdown();
        cache
    }
}

struct ItemBakeScope;

impl Drop for ItemBakeScope {
    fn drop(&mut self) {
        petramond_world::block::item_shape_bake::clear();
    }
}
