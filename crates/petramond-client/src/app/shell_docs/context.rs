//! What a shell screen controller is handed instead of the whole `App`: the
//! shell's own state, the options being edited, its document runtime, a
//! read-only view of the live session, and a queue for app-level requests
//! (screen transitions, starting or leaving a session, applying an option to
//! the audio mixer or the live game). The App carries the requests out once
//! the screen's frame has been dispatched.

use petramond::net::handle::ServerHandle;
use petramond::net::handshake::HandshakeJoin;
use petramond_input::controls::ActionTable;

use crate::app::options_state::OptionsState;
use crate::app::shell_state::ShellState;
use crate::app::ui_runtime::AppUi;
use crate::app::AppScreen;

/// Facts about the live game session a shell screen may show.
#[derive(Copy, Clone, Default)]
pub(in crate::app) struct SessionFacts<'a> {
    /// A game session exists (the options flow dims over it).
    pub in_game: bool,
    /// The session fronts a REMOTE server.
    pub is_remote: bool,
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
    /// Switch screens and hand the pointer to the menu (release any grab,
    /// recenter the cursor).
    Goto(AppScreen),
    /// Switch screens leaving the pointer as it is (moving within a flow
    /// whose pointer is already the menu's).
    SwitchTo(AppScreen),
    Quit,
    PlaySelectedWorld,
    /// Open (or create) the world saved under `dir_name`.
    StartGame {
        dir_name: String,
        seed: u32,
    },
    OpenConnectServer,
    ReopenConnectServer,
    BeginConnect,
    /// The connect worker's handshake succeeded: enter the remote session.
    AdoptRemote(Box<HandshakeJoin>, ServerHandle),
    OpenOptions {
        from_pause: bool,
    },
    CloseOptionsRoot,
    CloseOptionsCategory,
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
    pub action_table: &'a ActionTable,
    pub ui: &'a mut AppUi,
    pub session: SessionFacts<'a>,
    commands: Vec<ShellCommand>,
}

impl<'a> ScreenCtx<'a> {
    pub(in crate::app) fn new(
        shell: &'a mut ShellState,
        options: &'a mut OptionsState,
        action_table: &'a ActionTable,
        ui: &'a mut AppUi,
        session: SessionFacts<'a>,
    ) -> Self {
        Self {
            shell,
            options,
            action_table,
            ui,
            session,
            commands: Vec::new(),
        }
    }

    /// Queue an app-level request.
    pub(in crate::app) fn request(&mut self, command: ShellCommand) {
        self.commands.push(command);
    }

    /// Queue a screen switch that hands the pointer to the menu.
    pub(in crate::app) fn goto(&mut self, screen: AppScreen) {
        self.request(ShellCommand::Goto(screen));
    }

    /// The requests queued this frame, in order.
    pub(in crate::app) fn into_commands(self) -> Vec<ShellCommand> {
        self.commands
    }
}
