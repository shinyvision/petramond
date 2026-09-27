//! What a shell screen controller is handed instead of the whole `App`: the
//! shell's own state, the options being edited, the content library, its
//! document runtime, a read-only view of the live session, the presentation
//! time, and a queue for app-level requests
//! (screen transitions, starting or leaving a session, applying an option to
//! the audio mixer or the live game). The App carries the requests out once
//! the screen's frame has been dispatched.

use petramond::net::handle::ServerHandle;
use petramond::net::handshake::HandshakeJoin;
use petramond_input::controls::ActionTable;

use crate::app::content::ContentSession;
use crate::app::options_state::OptionsState;
use crate::app::shell_state::ShellState;
use crate::app::ui_runtime::AppUi;
use crate::app::{AppScreen, ExitKind};

/// Facts about the live game session a shell screen may show.
#[derive(Copy, Clone, Default)]
pub(in crate::app) struct SessionFacts<'a> {
    /// A game session exists (the options flow dims over it).
    pub in_game: bool,
    /// The session fronts a REMOTE server.
    pub is_remote: bool,
    /// The session is a presentation a launched pack opened: watched, not
    /// played — nothing to save and no LAN to open.
    pub presenting: bool,
    /// The port the HOST session is open to LAN on.
    pub lan_port: Option<u16>,
    /// The last Open to LAN failure.
    pub lan_error: Option<&'a str>,
    /// `(sleeping, total)` players, the local one included.
    pub sleep_counts: (usize, usize),
    /// The local sleep fade in `[0, 1]`, while asleep.
    pub sleep_progress: Option<f32>,
}

/// An app-level request a controller cannot carry out on the state it holds.
pub(in crate::app) enum ShellCommand {
    /// Switch screens (through the screen funnel, which applies the new
    /// screen's cursor policy), replacing any stacked screens.
    Goto(AppScreen),
    /// Open a screen over the current one; Back returns to it.
    Push(AppScreen),
    /// The screen's own Back: to the screen underneath, or its parent.
    Back,
    /// End the process through the one exit gate (running downloads ask
    /// first).
    Exit(ExitKind),
    /// End the process now, whatever runs.
    ExitNow(ExitKind),
    PlaySelectedWorld,
    /// Open (or create) the world saved under `dir_name`.
    StartGame {
        dir_name: String,
        seed: u32,
    },
    OpenConnectServer,
    BeginConnect,
    /// Leave Missing Mods for where the player came from.
    LeaveModsMissing,
    /// Open the content browser; `back` is the start route its Back (and a
    /// relaunch) returns to, `filter` the pack ids it lists.
    OpenContent {
        back: Option<String>,
        filter: Option<Vec<String>>,
    },
    CloseContent,
    ApplyContent,
    /// Open the Account screen, with a status line to show.
    OpenAccount(Option<String>),
    /// Leave the Account screen for the screen that opened it.
    LeaveAccount,
    OpenAccountSignIn,
    LeaveAccountSignIn,
    SubmitAccountSignIn,
    AccountSignOut,
    /// Start a pack's title-screen launch entry on the shell, at the
    /// window's `screen` size.
    LaunchPack {
        pack_id: String,
        screen: (u32, u32),
    },
    /// Close the presentation on screen, back to its owner.
    EndPresentation,
    /// The connect worker's handshake succeeded: enter the remote session.
    AdoptRemote(Box<HandshakeJoin>, ServerHandle),
    ResumeGame,
    OpenLan,
    DisconnectToTitle,
    SaveAndQuitToTitle,
    CancelSleep,
    Respawn,
    /// Push the options' volumes into the audio mixer.
    ApplyVolumes,
    /// Apply the options' particle mode to the live game.
    ApplyParticles,
    /// Apply a committed view distance to the live session and the next.
    ApplyViewDistance(i32),
}

pub(in crate::app) struct ScreenCtx<'a> {
    pub shell: &'a mut ShellState,
    pub options: &'a mut OptionsState,
    pub content: &'a mut ContentSession,
    /// What the startup apply of pending content changes did.
    pub content_report: &'a petramond::content::ApplyReport,
    pub action_table: &'a ActionTable,
    pub ui: &'a mut AppUi,
    pub session: SessionFacts<'a>,
    /// Presentation time (the wall, or a stepped clock's timeline).
    pub now: f64,
    commands: Vec<ShellCommand>,
}

impl<'a> ScreenCtx<'a> {
    pub(in crate::app) fn new(
        shell: &'a mut ShellState,
        options: &'a mut OptionsState,
        content: (&'a mut ContentSession, &'a petramond::content::ApplyReport),
        action_table: &'a ActionTable,
        ui: &'a mut AppUi,
        session: SessionFacts<'a>,
        now: f64,
    ) -> Self {
        Self {
            shell,
            options,
            content: content.0,
            content_report: content.1,
            action_table,
            ui,
            session,
            now,
            commands: Vec::new(),
        }
    }

    /// Queue an app-level request.
    pub(in crate::app) fn request(&mut self, command: ShellCommand) {
        self.commands.push(command);
    }

    /// Queue a screen switch.
    pub(in crate::app) fn goto(&mut self, screen: AppScreen) {
        self.request(ShellCommand::Goto(screen));
    }

    /// The requests queued this frame, in order.
    pub(in crate::app) fn into_commands(self) -> Vec<ShellCommand> {
        self.commands
    }
}
