use super::schematic_library::LibraryPage;
use super::screen::Escape;
use super::{App, AppScreen};

impl App {
    pub(super) fn set_screen(&mut self, next: AppScreen) {
        self.leave_screen(self.screen);
        while let Some(buried) = self.screens_under.pop() {
            self.leave_screen(buried);
        }
        self.enter_screen(next);
    }

    pub(super) fn push_screen(&mut self, next: AppScreen) {
        self.screens_under.push(self.screen);
        self.enter_screen(next);
    }

    pub(super) fn back_to(&mut self, parent: AppScreen) {
        match self.screens_under.pop() {
            Some(under) => {
                self.leave_screen(self.screen);
                self.enter_screen(under);
            }
            None => self.set_screen(parent),
        }
    }

    pub(super) fn go_back(&mut self) {
        if let Some(parent) = self.screen.spec().escape.back_target() {
            self.back_to(parent);
        }
    }

    pub(super) fn pause_open(&self) -> bool {
        self.screen == AppScreen::Pause || self.screens_under.contains(&AppScreen::Pause)
    }

    pub(super) fn close_screen(&mut self) -> bool {
        match self.screen.spec().escape {
            Escape::Unhandled => return false,
            Escape::Blocked => {}
            Escape::Back(parent) => self.back_to(parent),
            Escape::CancelRemapOrBack(parent) => {
                if self.options.remap().is_some() {
                    self.options.cancel_remap();
                } else {
                    self.back_to(parent);
                }
            }
            Escape::CancelConnect => {
                self.shell.connect.cancel();
                self.set_screen(AppScreen::Title);
            }
            Escape::LeaveModsMissing => self.leave_mods_missing(),
            Escape::LeaveAccount => self.leave_account(),
            Escape::LeaveAccountSignIn => self.leave_account_sign_in(),
            Escape::LeaveContent => self.content_escape(),
            Escape::LeaveClientUi => {
                if !self.dismiss_client_doc() {
                    self.leave_client_screen();
                    self.settle_shell();
                }
            }
            Escape::CloseMenu => self.close_menu(),
            Escape::CancelSleep => self.cancel_sleep(),
            Escape::PauseGame => {
                let tool_cancelled = self
                    .session
                    .as_mut()
                    .is_some_and(|session| session.game.cancel_world_tools());
                if !tool_cancelled {
                    self.open_pause();
                }
            }
            Escape::ResumeGame => self.resume_game(),
        }
        true
    }

    fn leave_screen(&mut self, screen: AppScreen) {
        match screen {
            AppScreen::Chat => {
                let now = self.now();
                if let Some(session) = self.session.as_mut() {
                    session.chat.clear_draft(now);
                }
            }
            AppScreen::ClientCanvas => self.client_canvas = None,
            AppScreen::Pause => {
                if let Some(session) = self.session.as_mut() {
                    session.lan_error = None;
                }
            }
            AppScreen::OptionsSound | AppScreen::OptionsControls | AppScreen::OptionsGraphics => {
                self.options.cancel_remap();
                self.options.clear_previews();
            }
            AppScreen::WorldSettings | AppScreen::CreateWorld | AppScreen::DeleteWorld => {
                self.shell.close_page();
            }
            AppScreen::ModsMissing => self.shell.missing_world = None,
            _ => {}
        }
    }

    fn enter_screen(&mut self, next: AppScreen) {
        self.screen = next;
        if next.spec().grabs_cursor() {
            self.controls.pointer.grab_for_gameplay();
        } else {
            self.controls.pointer.release_for_menu();
        }
        self.gui_router.reset_click_streak();
        self.crafting_browser.reset();
        let now = self.now();
        if let Some(session) = self.session.as_mut() {
            session.library_form.pending_delete = None;
            match next {
                AppScreen::Chat => session.chat.clear_draft(now),
                AppScreen::Schematics => session.library_form.page = LibraryPage::Library,
                _ => {}
            }
        }
    }
}
