use petramond_render::Renderer;

use super::{App, AppScreen};

impl App {
    fn viewport_rect(&self) -> Option<[u32; 4]> {
        if !matches!(self.screen, AppScreen::ClientModGui(_)) {
            return None;
        }
        let (_, r) = self.ui.out().viewports.first()?;
        (r.w > 0 && r.h > 0).then(|| [r.x.max(0) as u32, r.y.max(0) as u32, r.w as u32, r.h as u32])
    }

    fn wanted_frame_size(&self, window: (u32, u32)) -> Option<(u32, u32)> {
        let claim = self
            .session
            .as_ref()
            .and_then(|session| session.game.view_frame_size())
            .map(|[w, h]| (w, h));
        match self.viewport_rect() {
            Some(_) => Some(claim.unwrap_or(window)),
            None => claim,
        }
    }

    pub(super) fn drive_frame_size(&mut self, renderer: &mut Renderer) {
        let window = {
            let viewport = renderer.window_ui_viewport();
            viewport.size
        };
        let want = self
            .session
            .as_ref()
            .and_then(|_| self.wanted_frame_size(window));
        renderer.set_frame_destination(self.viewport_rect());
        if renderer.sized_frame_size() == want {
            self.set_frame_size = want;
            return;
        }
        match want {
            Some((w, h)) => match renderer.begin_sized_frames(w, h) {
                Ok(()) => {
                    self.set_frame_size = want;
                    self.apply_aspect(w, h);
                }
                Err(e) => log::warn!("the world frame cannot render at {w}x{h}: {e}"),
            },
            None => {
                renderer.end_sized_frames();
                self.set_frame_size = None;
                let (w, h) = renderer.screen_size();
                self.apply_aspect(w, h);
            }
        }
    }

    pub(super) fn doc_presents_world(kind: petramond_world::gui_state::GuiKind) -> bool {
        petramond::gui::documents::doc_for(kind).is_some_and(|doc| {
            let mut found = false;
            doc.doc.root.visit(&mut |node| {
                found |= matches!(node.kind, petramond_ui::NodeKind::Viewport { .. });
            });
            found
        })
    }
}
