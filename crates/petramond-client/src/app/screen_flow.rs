//! The screen funnel: every screen change goes through [`App::set_screen`],
//! [`App::push_screen`] or [`App::back_to`], which apply the new screen's
//! cursor policy and reset the per-screen transient state (click streak,
//! crafting browser, pending confirmations, chat draft, the client canvas,
//! options previews, the shell's page session) — so no transition has to
//! remember any of it. ESC is the table's [`Escape`] column, carried out by
//! [`App::close_screen`].
//!
//! Screens can stack: the Options flow is pushed over the title or the pause
//! menu (and its categories over the root), so Back pops to wherever the flow
//! began. [`App::set_screen`] replaces the whole stack.

use super::schematic_library::LibraryPage;
use super::screen::Escape;
use super::{now_seconds, App, AppScreen};

impl App {
    /// Switch to `next`, leaving the current screen and everything stacked
    /// under it.
    pub(super) fn set_screen(&mut self, next: AppScreen) {
        self.leave_screen(self.screen);
        while let Some(buried) = self.screens_under.pop() {
            self.leave_screen(buried);
        }
        self.enter_screen(next);
    }

    /// Open `next` over the current screen, which a Back returns to intact.
    pub(super) fn push_screen(&mut self, next: AppScreen) {
        self.screens_under.push(self.screen);
        self.enter_screen(next);
    }

    /// Leave the current screen for the one pushed underneath it, or for
    /// `parent` when nothing was.
    pub(super) fn back_to(&mut self, parent: AppScreen) {
        match self.screens_under.pop() {
            Some(under) => {
                self.leave_screen(self.screen);
                self.enter_screen(under);
            }
            None => self.set_screen(parent),
        }
    }

    /// A screen's own Back button: the same way out ESC takes, minus any
    /// ESC-only step (disarming a remap).
    pub(super) fn go_back(&mut self) {
        if let Some(parent) = self.screen.spec().escape.back_target() {
            self.back_to(parent);
        }
    }

    /// The pause menu is up, directly or under the Options flow pushed over
    /// it.
    pub(super) fn pause_open(&self) -> bool {
        self.screen == AppScreen::Pause || self.screens_under.contains(&AppScreen::Pause)
    }

    /// The close-screen control (ESC): the current screen's [`Escape`].
    /// Returns false only when there is nothing to close (the title screen),
    /// so the host may use the key itself.
    pub(super) fn close_screen(&mut self) -> bool {
        match self.screen.spec().escape {
            Escape::Unhandled => return false,
            // Death cannot be escaped — only the screen's buttons leave.
            Escape::Blocked => {}
            Escape::Back(parent) => self.back_to(parent),
            Escape::CancelRemapOrBack(parent) => {
                // ESC while a remap is armed only cancels the remap (the
                // raw-input capture path normally eats ESC first; this covers
                // direct control dispatch, e.g. tests).
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
            // Back to the connect screen, attempted address preserved.
            Escape::ReopenConnect => self.reopen_connect_server(),
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

    /// Per-screen transient state that must not outlive the screen.
    fn leave_screen(&mut self, screen: AppScreen) {
        match screen {
            AppScreen::Chat => {
                if let Some(session) = self.session.as_mut() {
                    session.chat.clear_draft(now_seconds());
                }
            }
            AppScreen::ClientCanvas => {
                if let Some(session) = self.session.as_mut() {
                    session.client_canvas = None;
                }
            }
            // A stale LAN error must not greet the next open.
            AppScreen::Pause => {
                if let Some(session) = self.session.as_mut() {
                    session.lan_error = None;
                }
            }
            // Leaving a category disarms any pending remap and drops its
            // unapplied slider previews.
            AppScreen::OptionsSound | AppScreen::OptionsControls | AppScreen::OptionsGraphics => {
                self.options.cancel_remap();
                self.options.clear_previews();
            }
            AppScreen::WorldSettings | AppScreen::CreateWorld | AppScreen::DeleteWorld => {
                self.shell.close_page();
            }
            _ => {}
        }
    }

    /// Make `next` current: its cursor policy, and a clean slate for the
    /// state every screen starts fresh with.
    fn enter_screen(&mut self, next: AppScreen) {
        self.screen = next;
        if next.spec().grabs_cursor() {
            self.controls.pointer.grab_for_gameplay();
        } else {
            // Show + recenter the cursor next tick; no held button carries
            // over from the screen before.
            self.controls.pointer.release_for_menu();
        }
        // No phantom double-click across screens.
        self.gui_router.reset_click_streak();
        self.crafting_browser.reset();
        if let Some(session) = self.session.as_mut() {
            // A delete confirmation is captured for the screen that asked.
            session.library_form.pending_delete = None;
            match next {
                AppScreen::Chat => session.chat.clear_draft(now_seconds()),
                AppScreen::Schematics => session.library_form.page = LibraryPage::Library,
                _ => {}
            }
        }
    }
}
