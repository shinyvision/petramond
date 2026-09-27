use std::sync::mpsc::Receiver;

use petramond::capture::body::NameTables;
use petramond::modding::client::present::Request;
use petramond::modding::client::ClientModRuntime;

use super::client_mod_ui::{client_canvas_owned_by, client_gui_owned_by, ClientCanvasState};
use super::session::Session;
use super::{App, AppScreen};

pub(super) struct LaunchedShell {
    runtime: Option<ClientModRuntime>,
    screen: AppScreen,
    canvas: Option<ClientCanvasState>,
    opening: Option<(Request, Receiver<Result<NameTables, String>>)>,
}

pub(super) fn launch_entries(
) -> impl Iterator<Item = (&'static str, &'static petramond_world::assets::LaunchEntry)> {
    petramond_world::assets::packs()
        .iter()
        .filter_map(|pack| Some((pack.id.as_deref()?, pack.launch.as_ref()?)))
}

impl App {
    pub(super) fn launch_pack(&mut self, pack_id: &str, screen: (u32, u32)) {
        if self.session.is_some() {
            return;
        }
        match ClientModRuntime::launch(pack_id) {
            Ok(runtime) => self.host_launched(runtime, screen),
            Err(e) => log::error!("launch '{pack_id}': {e}"),
        }
    }

    pub(super) fn host_launched(&mut self, runtime: ClientModRuntime, screen: (u32, u32)) {
        let pack_id = runtime.launched().unwrap_or_default().to_owned();
        self.launched = Some(LaunchedShell {
            runtime: Some(runtime),
            screen: AppScreen::Title,
            canvas: None,
            opening: None,
        });
        self.drive_client_mod_frame(0.0, 0.0, petramond::gui::UiViewport::new(screen, 0), true);
        if self.launched.is_none() {
            log::warn!("launch '{pack_id}': the mod opened no UI; nothing to show");
        }
        self.rebuild_action_table();
    }

    pub(super) fn shell_mods(&self) -> Option<&ClientModRuntime> {
        self.launched
            .as_ref()
            .filter(|_| self.session.is_none())?
            .runtime
            .as_ref()
    }

    pub(super) fn shell_mods_mut(&mut self) -> Option<&mut ClientModRuntime> {
        if self.session.is_some() {
            return None;
        }
        self.launched.as_mut()?.runtime.as_mut()
    }

    pub(super) fn client_mods_now(&self) -> Option<&ClientModRuntime> {
        match self.session.as_ref() {
            Some(session) => Some(session.game.client_mod_runtime()),
            None => self.shell_mods(),
        }
    }

    pub(super) fn client_canvas_key(&self) -> Option<&str> {
        self.client_canvas
            .as_ref()
            .filter(|_| self.screen == AppScreen::ClientCanvas)
            .map(ClientCanvasState::key)
    }

    pub(super) fn leave_client_screen(&mut self) {
        self.client_canvas = None;
        let next = if self.session.is_some() {
            AppScreen::Game
        } else {
            AppScreen::Title
        };
        self.set_screen(next);
    }

    pub(super) fn settle_shell(&mut self) {
        let Some(runtime) = self.shell_mods() else {
            return;
        };
        let own = runtime.launched().is_some_and(|owner| {
            client_gui_owned_by(self.screen, owner)
                || client_canvas_owned_by(self.screen, owner, &self.client_canvas)
        });
        if own {
            return;
        }
        self.launched = None;
        if self.screen.client_ui_open() || self.screen.client_canvas_open() {
            self.leave_client_screen();
        }
        self.rebuild_action_table();
    }

    pub(super) fn drive_shell_presentation(&mut self) {
        let asked = match self.session.as_mut() {
            Some(session) => session.game.take_presentation_reopen(),
            None => self
                .shell_mods()
                .and_then(|runtime| runtime.presented().lock().presentation.take_open()),
        };
        if let (Some(open), Some(shell)) = (asked, self.launched.as_mut()) {
            if let Request::Open { tables, .. } = &open {
                let read = petramond::capture::present::open_tables(tables);
                shell.opening = Some((open, read));
            }
        }
        let Some(shell) = self.launched.as_mut() else {
            return;
        };
        let landed = match &shell.opening {
            Some((_, read)) => match read.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("the tables read never answered".into())
                }
            },
            None => return,
        };
        let (open, _) = shell.opening.take().expect("matched above");
        let Request::Open {
            seed, mods, viewer, ..
        } = open
        else {
            return;
        };
        match landed {
            Ok(tables) => self.start_presentation(tables, seed, mods, viewer),
            Err(why) => {
                log::warn!("presentation never opened: {why}");
                let desk = match self.session.as_ref() {
                    Some(session) => Some(session.game.client_mod_runtime().presented().clone()),
                    None => self.shell_mods().map(|r| r.presented().clone()),
                };
                if let Some(desk) = desk {
                    desk.lock().presentation.ended(Some(why));
                }
            }
        }
    }

    fn start_presentation(
        &mut self,
        tables: NameTables,
        seed: u32,
        mods: Vec<String>,
        viewer: Option<mod_api::ClientPose>,
    ) {
        let runtime = match self.session.take() {
            Some(Session { game, .. }) => {
                self.teardown_game_scene();
                game.into_shell()
            }
            None => {
                let Some(shell) = self.launched.as_mut() else {
                    return;
                };
                shell.screen = self.screen;
                shell.canvas = self.client_canvas.clone();
                shell.runtime.take()
            }
        };
        let Some(runtime) = runtime else {
            self.launched = None;
            self.set_screen(AppScreen::Title);
            return;
        };
        let enabled = mods.into_iter().collect();
        let cam = petramond_render::camera::Camera::new(
            petramond_math::world_pos::WorldPos::new(0.0, 80.0, 0.0),
            self.shell_camera.aspect.max(0.01),
        );
        self.session = Some(Session::new(crate::game::Game::new_presentation(
            cam,
            self.render_dist,
            seed,
            tables,
            viewer,
            runtime.host_presentation(seed, &enabled),
        )));
        self.apply_particles();
        self.rebuild_action_table();
        self.renderer_world_clear_pending = true;
    }

    pub(super) fn end_presentation(&mut self) {
        if !self
            .session
            .as_ref()
            .is_some_and(|session| session.game.in_presentation())
        {
            return;
        }
        let runtime = self
            .session
            .take()
            .and_then(|Session { game, .. }| game.leave_presentation());
        self.teardown_game_scene();
        match (runtime, self.launched.as_mut()) {
            (Some(runtime), Some(shell)) => {
                shell.runtime = Some(runtime);
                let (screen, canvas) = (shell.screen, shell.canvas.take());
                self.set_screen(screen);
                self.client_canvas = canvas;
            }
            _ => {
                self.launched = None;
                self.set_screen(AppScreen::Title);
            }
        }
        self.rebuild_action_table();
        self.settle_shell();
    }

    pub(super) fn client_mod_view(
        &self,
        kind_key: &str,
    ) -> Option<petramond::modding::client::ClientUiView> {
        match self.session.as_ref() {
            Some(session) => session.game.client_mod_view(kind_key),
            None => self.shell_mods()?.view_for(kind_key),
        }
    }

    pub(super) fn client_mod_canvas_view(
        &self,
        canvas_key: &str,
    ) -> Option<petramond::modding::client::ClientCanvasView> {
        match self.session.as_ref() {
            Some(session) => session.game.client_mod_canvas_view(canvas_key),
            None => self.shell_mods()?.canvas_view(canvas_key),
        }
    }

    pub(super) fn client_mod_ui_event(&mut self, kind_key: &str, event: mod_api::ClientUiEvent) {
        match self.session.as_mut() {
            Some(session) => session.game.client_mod_ui_event(kind_key, event),
            None => {
                if let Some(runtime) = self.shell_mods_mut() {
                    runtime.ui_event(None, kind_key, event);
                }
            }
        }
    }

    pub(super) fn client_mod_canvas_event(
        &mut self,
        canvas_key: &str,
        event: mod_api::ClientCanvasEvent,
    ) {
        match self.session.as_mut() {
            Some(session) => session.game.client_mod_canvas_event(canvas_key, event),
            None => {
                if let Some(runtime) = self.shell_mods_mut() {
                    runtime.canvas_event(None, canvas_key, event);
                }
            }
        }
    }

    pub(super) fn client_mod_canvas_scroll(
        &mut self,
        canvas_key: &str,
        x: f32,
        y: f32,
        delta: f32,
    ) {
        match self.session.as_mut() {
            Some(session) => session
                .game
                .client_mod_canvas_scroll(canvas_key, x, y, delta),
            None => {
                if let Some(runtime) = self.shell_mods_mut() {
                    runtime.canvas_scroll(None, canvas_key, x, y, delta);
                }
            }
        }
    }

    pub(super) fn take_client_mod_commands(&mut self) -> Vec<petramond::modding::ClientCommand> {
        match self.session.as_mut() {
            Some(session) => session.game.take_client_mod_commands(),
            None => self
                .shell_mods_mut()
                .map(ClientModRuntime::take_commands)
                .unwrap_or_default(),
        }
    }
}
