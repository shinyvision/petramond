use petramond::modding::client::ClientModRuntime;
use petramond::net::{handle::ServerHandle, handshake::HandshakeJoin};
use petramond_render::{camera::Camera, Renderer};
use petramond_world::content::{self, Content};

use super::{App, AppScreen};
use crate::game::Game;

pub(super) struct PendingStart {
    content: Content,
    request: Request,
}

enum Request {
    Local {
        world: String,
        seed: u32,
    },
    Remote {
        join: HandshakeJoin,
        handle: ServerHandle,
    },
    Launch {
        pack_id: String,
        screen: (u32, u32),
    },
}

pub(super) enum Started {
    Game {
        game: Box<Game>,
        auto_lan: bool,
    },
    Launch {
        runtime: Box<ClientModRuntime>,
        screen: (u32, u32),
        content: content::PinGuard,
    },
}

impl Request {
    fn title(&self) -> &'static str {
        match self {
            Self::Local { .. } => "Couldn't open",
            Self::Remote { .. } => "Couldn't connect",
            Self::Launch { .. } => "Couldn't launch",
        }
    }

    fn build(self, app: &mut App, selected: Content) -> Result<Started, String> {
        let camera = || {
            Camera::new(
                petramond_math::world_pos::WorldPos::new(8.0, 90.0, 8.0),
                app.shell_camera.aspect.max(0.01),
            )
        };
        match self {
            Self::Local { world, seed } => {
                let game = Game::new(camera(), &world, seed, app.render_dist)?;
                Ok(Started::Game {
                    game: Box::new(game),
                    auto_lan: petramond::save::read_world_settings(&world).auto_open_lan,
                })
            }
            Self::Remote { join, handle } => {
                let game = Game::new_remote(
                    camera(),
                    join.join,
                    handle,
                    app.render_dist,
                    &app.shell.connect.addr,
                    &join.server_mods,
                    app.retained_section_cache.take(),
                )?;
                Ok(Started::Game {
                    game: Box::new(game),
                    auto_lan: false,
                })
            }
            Self::Launch { pack_id, screen } => {
                let content = content::pin(selected);
                let runtime = ClientModRuntime::launch(&pack_id)?;
                Ok(Started::Launch {
                    runtime: Box::new(runtime),
                    screen,
                    content,
                })
            }
        }
    }
}

impl App {
    pub(super) fn stage_local_start(&mut self, world: &str, seed: u32) {
        let request = Request::Local {
            world: world.into(),
            seed,
        };
        let selected = if world.is_empty() {
            Ok(Content::current())
        } else {
            petramond::content::for_world(
                &petramond::save::read_world_settings(world).disabled_mods,
            )
        };
        self.stage_start(request, selected.map_err(|error| error.to_string()));
    }

    pub(super) fn stage_remote_start(&mut self, join: HandshakeJoin, handle: ServerHandle) {
        let mut enabled = join.server_mods.clone();
        if join.join.client_policy.presentation_packs {
            enabled.extend(petramond::modding::client::presentation_only_packs());
        }
        let selected = petramond::content::for_enabled(&enabled).map_err(|error| error.to_string());
        self.stage_start(Request::Remote { join, handle }, selected);
    }

    pub(super) fn stage_launch(&mut self, pack_id: &str, screen: (u32, u32)) {
        let selected =
            petramond::content::for_world(&Default::default()).map_err(|error| error.to_string());
        self.stage_start(
            Request::Launch {
                pack_id: pack_id.into(),
                screen,
            },
            selected,
        );
    }

    fn stage_start(&mut self, request: Request, selected: Result<Content, String>) {
        if self.session.is_some() {
            return;
        }
        self.pending_start = None;
        match selected {
            Ok(content) => self.pending_start = Some(PendingStart { content, request }),
            Err(error) => self.report_start_error(request.title(), error),
        }
    }

    pub(super) fn prepare_pending<P>(
        &mut self,
        prepare: impl FnOnce(Content) -> Result<P, String>,
    ) -> Option<(Started, P)> {
        let PendingStart { content, request } = self.pending_start.take()?;
        let title = request.title();
        let prepared = match prepare(content) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.report_start_error(title, error);
                return None;
            }
        };
        match request.build(self, content) {
            Ok(started) if Content::current().same(content) => Some((started, prepared)),
            Ok(started) => {
                drop(started);
                self.report_start_error(
                    title,
                    "The enabled packs changed while opening. Please try again.".into(),
                );
                None
            }
            Err(error) => {
                self.report_start_error(title, error);
                None
            }
        }
    }

    pub(super) fn drive_pending_start(&mut self, renderer: &mut Renderer) {
        if let Some((started, prepared)) = self.prepare_pending(|content| {
            renderer
                .prepare_content(content)
                .map_err(|error| error.to_string())
        }) {
            if let Some(prepared) = prepared {
                renderer.commit_content(prepared);
            }
            self.activate_start(started);
            self.renderer_recreated(renderer);
        }
    }

    pub(super) fn activate_start(&mut self, started: Started) {
        match started {
            Started::Game { game, auto_lan } => {
                self.adopt_game(*game);
                if auto_lan {
                    self.open_lan();
                }
            }
            Started::Launch {
                runtime,
                screen,
                content,
            } => {
                self.host_launched(*runtime, screen);
                if let Some(shell) = self.launched.as_mut() {
                    shell.set_content(content);
                }
            }
        }
        self.sync_content();
    }

    pub(super) fn report_start_error(&mut self, title: &str, error: String) {
        log::error!("{title}: {error}");
        self.shell.set_session_failure(title.into(), error);
        self.set_screen(AppScreen::ConnectionLost);
    }
}
