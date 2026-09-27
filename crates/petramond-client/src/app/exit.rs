use super::content::StartRoute;
use super::App;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitKind {
    Quit,
    Restart { route: StartRoute },
}

impl App {
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

    pub(super) fn exit_now(&mut self, kind: ExitKind) {
        self.content.jobs.cancel_all();
        if let ExitKind::Restart { route } = kind {
            self.relaunch = Some(route.to_json());
        }
        self.quit_requested = true;
    }

    pub fn take_relaunch(&mut self) -> Option<String> {
        self.relaunch.take()
    }
}
