use super::App;

impl App {
    pub(super) fn refresh_renderer_content(
        &mut self,
        renderer: &mut petramond_render::Renderer,
    ) -> Result<(), petramond_render::RenderInitError> {
        if let Some(prepared) =
            renderer.prepare_content(petramond_world::content::Content::current())?
        {
            renderer.commit_content(prepared);
            self.renderer_recreated(renderer);
        }
        self.sync_content();
        Ok(())
    }

    pub(super) fn sync_content(&mut self) {
        let serial = petramond_world::content::current().serial();
        if self.content_serial == serial {
            return;
        }
        self.content_serial = serial;
        self.ui = super::ui_runtime::AppUi::new();
        self.hud_ui = super::ui_runtime::AppUi::new();
        self.composed_doc = Default::default();
        self.composed_doc_images.clear();
        self.composed_hud = Default::default();
        self.composed_hud_images.clear();
        self.player_view = None;
        self.sound.reload_content();
    }
}
