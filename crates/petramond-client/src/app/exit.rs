//! How the process ends: the ONE gate every way of ending goes through (the
//! title's Quit, the window's close request, Escape on the title, a restart
//! to apply content changes), and the relaunch a restart asks for.
//!
//! With downloads running, the gate asks first: the content browser opens on
//! its exit confirm (quit now, or once the queue drains).

use super::content::StartRoute;
use super::App;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitKind {
    Quit,
    /// Quit, then start again into `route` (a `PETRAMOND_START` value).
    Restart {
        route: StartRoute,
    },
}

impl App {
    /// End the process the way `kind` says, through the ordinary quit path
    /// (every setting flushes exactly as on Quit) — unless downloads are
    /// running, which the player is asked about first.
    pub fn request_exit(&mut self, kind: ExitKind) {
        if !self.content_jobs_running() {
            return self.exit_now(kind);
        }
        if let Some(session) = self.session.as_ref() {
            if session.game.is_remote() {
                self.disconnect_to_title();
            } else {
                self.save_and_quit_to_title();
            }
        }
        if !matches!(self.screen, super::AppScreen::Content) {
            self.open_content(None, None);
        }
        self.confirm_content_exit(kind);
    }

    /// End now, whatever runs.
    pub(super) fn exit_now(&mut self, kind: ExitKind) {
        self.content.jobs.cancel_all();
        if let ExitKind::Restart { route } = kind {
            self.relaunch = Some(route.to_json());
        }
        self.quit_requested = true;
    }

    /// The start route a restart asked to relaunch into, once the event loop
    /// has ended.
    pub fn take_relaunch(&mut self) -> Option<String> {
        self.relaunch.take()
    }
}
