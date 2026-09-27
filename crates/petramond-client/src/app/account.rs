use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::{App, AppScreen};
use petramond::account::{self, AccountError, SavedSignIn};

enum AccountOutcome {
    Done,
    Failed(String),
}

#[derive(Default)]
pub(super) struct AccountSession {
    pub(super) saved: Option<SavedSignIn>,
    pub(super) status: String,
    pub(super) busy: Option<&'static str>,
    pub(super) return_to: Option<AppScreen>,
    gen: u64,
    rx: Option<Receiver<(u64, AccountOutcome)>>,
}

impl AccountSession {
    pub(super) fn working(&self) -> bool {
        self.busy.is_some()
    }

    fn reload(&mut self) {
        self.saved = account::session::current();
    }
}

impl App {
    pub(super) fn refresh_account_view(&mut self) {
        self.shell.account.reload();
    }

    pub(super) fn open_account(&mut self, status: Option<String>) {
        self.note_account_opener();
        self.shell.account.cancel();
        self.shell.account.reload();
        self.shell.account.status = status.unwrap_or_default();
        self.set_screen(AppScreen::Account);
        if self.shell.account.saved.is_some() && !cfg!(test) {
            self.spawn_account_request("Checking your sign-in…", account::session::verify);
        }
    }

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

    pub(super) fn leave_account_sign_in(&mut self) {
        if self.shell.account.return_to == Some(AppScreen::Content) {
            self.shell.account.return_to = None;
            self.shell.account.cancel();
            self.set_screen(AppScreen::Content);
        } else {
            self.open_account(None);
        }
    }

    pub(super) fn leave_account(&mut self) {
        let back = self
            .shell
            .account
            .return_to
            .take()
            .unwrap_or(AppScreen::Title);
        self.set_screen(back);
    }

    fn note_account_opener(&mut self) {
        if !matches!(self.screen, AppScreen::Account | AppScreen::AccountSignIn) {
            self.shell.account.return_to = Some(self.screen);
        }
    }

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

    pub(super) fn account_sign_out(&mut self) {
        if self.shell.account.working() {
            return;
        }
        self.spawn_account_request("Signing out…", || {
            account::session::sign_out();
            Ok(())
        });
    }

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
    fn cancel(&mut self) {
        self.gen += 1;
        self.rx = None;
        self.busy = None;
    }

    pub(super) fn poll(&mut self) -> bool {
        loop {
            let Some(rx) = self.rx.as_ref() else {
                return false;
            };
            let (gen, outcome) = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
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

const ID_MAX_CHARS: usize = 254;
