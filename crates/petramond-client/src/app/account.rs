//! The Account flow: the cached view of the stored Petramond sign-in,
//! and the ONE background thread per account request (sign in, confirm, sign
//! out). Every request blocks on the network, so none of it runs on the frame.
//!
//! The cache exists because binding runs EVERY FRAME: reading the credential
//! file thirty times a second to draw one label would be the only disk traffic
//! on an idle menu. `saved` is refreshed when it can have changed — entering the
//! screen, and each worker outcome — and read freely in between.
//!
//! Cancellation follows the connect worker's shape (`super::connect`): a
//! GENERATION guard makes an abandoned request's outcome stale rather than
//! racing the screen it no longer belongs to.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::{App, AppScreen};
use petramond::account::{self, AccountError, SavedSignIn};

/// What an account request reports back. The request itself has already written
/// whatever it changed, so the outcome only decides what the screen says next.
enum AccountOutcome {
    Done,
    /// The message fills the screen's status label.
    Failed(String),
}

#[derive(Default)]
pub(super) struct AccountSession {
    /// The stored sign-in as of the last time it could have changed. `None` =
    /// nobody is signed in.
    pub(super) saved: Option<SavedSignIn>,
    /// The last message to show the player (a refusal, or why they were sent
    /// here from a join).
    pub(super) status: String,
    /// The running request's progress label, if any.
    pub(super) busy: Option<&'static str>,
    /// Where Back leads: the screen that opened the account screens.
    pub(super) return_to: Option<AppScreen>,
    gen: u64,
    rx: Option<Receiver<(u64, AccountOutcome)>>,
}

impl AccountSession {
    pub(super) fn working(&self) -> bool {
        self.busy.is_some()
    }

    /// Re-read the stored sign-in. Called only where it can have changed.
    fn reload(&mut self) {
        self.saved = account::session::current();
    }
}

impl App {
    /// Re-read the stored sign-in into the cached view. Called where a screen is
    /// about to bind it and it could have changed since the last look.
    pub(super) fn refresh_account_view(&mut self) {
        self.shell.account.reload();
    }

    /// Open Account. `status` carries a reason the player was sent here (a
    /// join that needed a sign-in), shown in place of a stale message.
    pub(super) fn open_account(&mut self, status: Option<String>) {
        self.note_account_opener();
        self.shell.account.cancel();
        self.shell.account.reload();
        self.shell.account.status = status.unwrap_or_default();
        self.set_screen(AppScreen::Account);
        // Confirm the stored sign-in against the service while the screen is up:
        // it also rotates a due token and picks up a rename, so what the screen
        // shows is what a server would see.
        //
        // Suppressed under test for the same reason `connect::remember_server`
        // is: the suite must never touch the DEVELOPER's real account — and
        // here that would also mean live HTTPS calls out of a unit test.
        if self.shell.account.saved.is_some() && !cfg!(test) {
            self.spawn_account_request("Checking your sign-in…", account::session::verify);
        }
    }

    /// Open the password form, empty and focused.
    pub(super) fn open_account_sign_in(&mut self) {
        self.note_account_opener();
        self.shell.account.cancel();
        self.shell.account.status.clear();
        self.ui
            .ensure_active(petramond_world::gui_state::GuiKind::AccountSignIn);
        let state = self.ui.state_mut();
        state.set("account_id", petramond_ui::UiValue::Str(String::new()));
        state.set(
            "account_password",
            petramond_ui::UiValue::Str(String::new()),
        );
        self.ui.focus_text_input("account_id", "", ID_MAX_CHARS);
        self.set_screen(AppScreen::AccountSignIn);
    }

    /// Leave the password form: back to the content browser when that is
    /// what asked for a sign-in, else to the Account overview.
    pub(super) fn leave_account_sign_in(&mut self) {
        if self.shell.account.return_to == Some(AppScreen::Content) {
            self.shell.account.return_to = None;
            self.shell.account.cancel();
            self.set_screen(AppScreen::Content);
        } else {
            self.open_account(None);
        }
    }

    /// Leave the Account overview for the screen that opened it.
    pub(super) fn leave_account(&mut self) {
        let back = self
            .shell
            .account
            .return_to
            .take()
            .unwrap_or(AppScreen::Title);
        self.set_screen(back);
    }

    /// Remember where the account screens were opened from; moving between
    /// the two of them keeps it.
    fn note_account_opener(&mut self) {
        if !matches!(self.screen, AppScreen::Account | AppScreen::AccountSignIn) {
            self.shell.account.return_to = Some(self.screen);
        }
    }

    /// Submit the password form.
    pub(super) fn submit_account_sign_in(&mut self) {
        if self.shell.account.working() {
            return;
        }
        let state = self.ui.state_mut();
        let identifier = state.get_str("account_id").unwrap_or("").trim().to_owned();
        let password = state.get_str("account_password").unwrap_or("").to_owned();
        if identifier.is_empty() || password.is_empty() {
            self.shell.account.status = "Enter your Petramond username and password".to_owned();
            return;
        }
        self.spawn_account_request("Signing in…", move || {
            account::session::sign_in(&identifier, &password)
        });
    }

    /// Sign out: forget the stored credential and tell the service to drop it.
    pub(super) fn account_sign_out(&mut self) {
        if self.shell.account.working() {
            return;
        }
        self.spawn_account_request("Signing out…", || {
            account::session::sign_out();
            Ok(())
        });
    }

    /// Run one blocking account call on its own thread under the current
    /// generation. `request` returns the identity it established, or why not.
    fn spawn_account_request<T: Send + 'static>(
        &mut self,
        label: &'static str,
        request: impl FnOnce() -> Result<T, AccountError> + Send + 'static,
    ) {
        self.shell.account.gen += 1;
        let gen = self.shell.account.gen;
        let (tx, rx) = mpsc::channel();
        self.shell.account.rx = Some(rx);
        self.shell.account.busy = Some(label);
        self.shell.account.status.clear();
        let spawned = std::thread::Builder::new()
            .name("petramond-account".to_owned())
            .spawn(move || {
                let outcome = match request() {
                    Ok(_) => AccountOutcome::Done,
                    Err(e) => AccountOutcome::Failed(e.message().to_owned()),
                };
                let _ = tx.send((gen, outcome));
            });
        if spawned.is_err() {
            self.shell.account.rx = None;
            self.shell.account.busy = None;
            self.shell.account.status = "Could not start the account request".to_owned();
        }
    }
}

impl AccountSession {
    /// Abandon any in-flight request: its outcome becomes stale by generation.
    fn cancel(&mut self) {
        self.gen += 1;
        self.rx = None;
        self.busy = None;
    }

    /// Drain the account worker — the Account screens' per-frame prep. `true`
    /// when a request completed and left a sign-in stored: a password form
    /// then has done its job. A failure stays put with the reason bound.
    pub(super) fn poll(&mut self) -> bool {
        loop {
            let Some(rx) = self.rx.as_ref() else {
                return false;
            };
            let (gen, outcome) = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    // The thread died without reporting (a panic): fail loud
                    // rather than spin on a progress label forever.
                    self.rx = None;
                    self.busy = None;
                    self.status = "The account request failed".to_owned();
                    self.reload();
                    return false;
                }
            };
            if gen != self.gen {
                continue;
            }
            self.rx = None;
            self.busy = None;
            // Every outcome can have changed the stored credential — a refusal
            // that says the token is dead cleared it (see `account::session`).
            self.reload();
            return match outcome {
                AccountOutcome::Done => {
                    self.status.clear();
                    self.saved.is_some()
                }
                AccountOutcome::Failed(message) => {
                    self.status = message;
                    false
                }
            };
        }
    }
}

/// Matches the document's `max_chars` for the identifier input.
const ID_MAX_CHARS: usize = 254;
