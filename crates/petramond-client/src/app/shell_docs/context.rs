use petramond::net::handle::ServerHandle;
use petramond::net::handshake::HandshakeJoin;
use petramond_input::controls::ActionTable;

use crate::app::content::ContentSession;
use crate::app::options_state::OptionsState;
use crate::app::shell_state::ShellState;
use crate::app::ui_runtime::AppUi;
use crate::app::{AppScreen, ExitKind};

#[derive(Copy, Clone, Default)]
pub(in crate::app) struct SessionFacts<'a> {
    pub in_game: bool,
    pub is_remote: bool,
    pub presenting: bool,
    pub lan_port: Option<u16>,
    pub lan_error: Option<&'a str>,
    pub sleep_counts: (usize, usize),
    pub sleep_progress: Option<f32>,
}

pub(in crate::app) enum ShellCommand {
    Goto(AppScreen),
    Push(AppScreen),
    Back,
    Exit(ExitKind),
    ExitNow(ExitKind),
    PlaySelectedWorld,
    StartGame {
        dir_name: String,
        seed: u32,
    },
    OpenConnectServer,
    BeginConnect,
    LeaveModsMissing,
    OpenContent {
        back: Option<String>,
        filter: Option<Vec<String>>,
    },
    CloseContent,
    ApplyContent,
    OpenAccount(Option<String>),
    LeaveAccount,
    OpenAccountSignIn,
    LeaveAccountSignIn,
    SubmitAccountSignIn,
    AccountSignOut,
    LaunchPack {
        pack_id: String,
        screen: (u32, u32),
    },
    EndPresentation,
    AdoptRemote(Box<HandshakeJoin>, ServerHandle),
    ResumeGame,
    OpenLan,
    DisconnectToTitle,
    SaveAndQuitToTitle,
    CancelSleep,
    Respawn,
    ApplyVolumes,
    ApplyParticles,
    ApplyViewDistance(i32),
}

pub(in crate::app) struct ScreenCtx<'a> {
    pub shell: &'a mut ShellState,
    pub options: &'a mut OptionsState,
    pub content: &'a mut ContentSession,
    pub content_report: &'a petramond::content::ApplyReport,
    pub action_table: &'a ActionTable,
    pub ui: &'a mut AppUi,
    pub session: SessionFacts<'a>,
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

    pub(in crate::app) fn request(&mut self, command: ShellCommand) {
        self.commands.push(command);
    }

    pub(in crate::app) fn goto(&mut self, screen: AppScreen) {
        self.request(ShellCommand::Goto(screen));
    }

    pub(in crate::app) fn into_commands(self) -> Vec<ShellCommand> {
        self.commands
    }
}
