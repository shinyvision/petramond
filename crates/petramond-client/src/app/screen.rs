use petramond_world::gui_state::GuiKind;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum AppScreen {
    Title,
    WorldSelect,
    WorldSettings,
    CreateWorld,
    DeleteWorld,
    ConnectServer,
    Account,
    AccountSignIn,
    Content,
    ModsMissing,
    ConnectionLost,
    Options,
    OptionsSound,
    OptionsControls,
    OptionsGraphics,
    Game,
    Chat,
    Pause,
    Menu(GuiKind),
    ClientModGui(GuiKind),
    Schematics,
    ClientCanvas,
    Sleeping,
    Dead,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ScreenRole {
    Gameplay,
    Chat,
    Shell,
    Overlay,
    GameMenu,
    ClientDoc,
    Canvas,
}

impl ScreenRole {
    #[inline]
    pub(super) fn runs_sim(self) -> bool {
        self != ScreenRole::Shell
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Escape {
    Unhandled,
    Blocked,
    Back(AppScreen),
    CancelRemapOrBack(AppScreen),
    CancelConnect,
    LeaveModsMissing,
    LeaveAccount,
    LeaveAccountSignIn,
    LeaveContent,
    LeaveClientUi,
    CloseMenu,
    CancelSleep,
    PauseGame,
    ResumeGame,
}

impl Escape {
    #[inline]
    pub(super) fn back_target(self) -> Option<AppScreen> {
        match self {
            Escape::Back(parent) | Escape::CancelRemapOrBack(parent) => Some(parent),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum HandPolicy {
    Shown,
    Hidden,
    SleepFade,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ScreenSpec {
    pub(super) role: ScreenRole,
    pub(super) doc: Option<GuiKind>,
    pub(super) hud: bool,
    pub(super) hand: HandPolicy,
    pub(super) escape: Escape,
}

impl ScreenSpec {
    fn new(role: ScreenRole, doc: Option<GuiKind>, escape: Escape) -> Self {
        Self {
            role,
            doc,
            hud: false,
            hand: HandPolicy::Shown,
            escape,
        }
    }

    fn with_hud(mut self) -> Self {
        self.hud = true;
        self
    }

    fn with_hand(mut self, hand: HandPolicy) -> Self {
        self.hand = hand;
        self
    }

    #[inline]
    pub(super) fn grabs_cursor(&self) -> bool {
        self.role == ScreenRole::Gameplay
    }
}

impl AppScreen {
    pub(super) fn spec(self) -> ScreenSpec {
        use AppScreen as S;
        use Escape as E;
        use ScreenRole as R;
        use ScreenSpec as Spec;
        let shell = |doc: GuiKind, escape: Escape| Spec::new(R::Shell, Some(doc), escape);
        match self {
            S::Title => shell(
                if std::env::var_os("PETRAMOND_UI_DEMO").is_some() {
                    GuiKind::Demo
                } else {
                    GuiKind::Title
                },
                E::Unhandled,
            ),
            S::WorldSelect => shell(GuiKind::WorldSelect, E::Back(S::Title)),
            S::WorldSettings => shell(GuiKind::WorldSettings, E::Back(S::WorldSelect)),
            S::CreateWorld => shell(GuiKind::CreateWorld, E::Back(S::WorldSelect)),
            S::DeleteWorld => shell(GuiKind::DeleteWorld, E::Back(S::WorldSelect)),
            S::ConnectServer => shell(GuiKind::ConnectServer, E::CancelConnect),
            S::Account => shell(GuiKind::Account, E::LeaveAccount),
            S::AccountSignIn => shell(GuiKind::AccountSignIn, E::LeaveAccountSignIn),
            S::Content => shell(GuiKind::Content, E::LeaveContent),
            S::ModsMissing => shell(GuiKind::ModsMissing, E::LeaveModsMissing),
            S::ConnectionLost => shell(GuiKind::ConnectionLost, E::Back(S::Title)),
            S::Options => shell(GuiKind::Options, E::Back(S::Title)),
            S::OptionsSound => shell(GuiKind::OptionsSound, E::CancelRemapOrBack(S::Options)),
            S::OptionsControls => shell(GuiKind::OptionsControls, E::CancelRemapOrBack(S::Options)),
            S::OptionsGraphics => shell(GuiKind::OptionsGraphics, E::CancelRemapOrBack(S::Options)),
            S::Pause => shell(GuiKind::Pause, E::ResumeGame).with_hand(HandPolicy::Hidden),
            S::Game => Spec::new(R::Gameplay, None, E::PauseGame).with_hud(),
            S::Chat => Spec::new(R::Chat, None, E::Back(S::Game)).with_hud(),
            S::Menu(kind) => Spec::new(R::GameMenu, Some(kind), E::CloseMenu),
            S::ClientModGui(kind) => Spec::new(R::ClientDoc, Some(kind), E::LeaveClientUi),
            S::Schematics => Spec::new(R::ClientDoc, Some(GuiKind::Schematics), E::LeaveClientUi),
            S::ClientCanvas => Spec::new(R::Canvas, None, E::LeaveClientUi),
            S::Sleeping => Spec::new(R::Overlay, Some(GuiKind::Sleep), E::CancelSleep)
                .with_hand(HandPolicy::SleepFade),
            S::Dead => Spec::new(R::Overlay, Some(GuiKind::Death), E::Blocked)
                .with_hand(HandPolicy::Hidden),
        }
    }

    #[inline]
    pub(super) fn role(self) -> ScreenRole {
        self.spec().role
    }

    #[inline]
    pub(super) fn gameplay_enabled(self) -> bool {
        self.role() == ScreenRole::Gameplay
    }

    #[inline]
    pub(super) fn window_only(self) -> bool {
        !matches!(self.role(), ScreenRole::Gameplay | ScreenRole::Overlay)
    }

    #[inline]
    pub(super) fn shell_open(self) -> bool {
        self.role() == ScreenRole::Shell
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn inventory_open(self) -> bool {
        self == AppScreen::Menu(GuiKind::Inventory)
    }

    #[inline]
    pub(super) fn ui_open(self) -> bool {
        self.role() == ScreenRole::GameMenu
    }

    #[inline]
    pub(super) fn client_ui_open(self) -> bool {
        self.role() == ScreenRole::ClientDoc
    }

    #[inline]
    pub(super) fn client_canvas_open(self) -> bool {
        self.role() == ScreenRole::Canvas
    }

    #[inline]
    pub(super) fn gui_kind(self) -> GuiKind {
        let spec = self.spec();
        match spec.role {
            ScreenRole::Gameplay | ScreenRole::Chat => GuiKind::Hotbar,
            ScreenRole::Shell | ScreenRole::Canvas => GuiKind::Other,
            ScreenRole::Overlay | ScreenRole::GameMenu | ScreenRole::ClientDoc => {
                spec.doc.unwrap_or(GuiKind::Other)
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CursorIcon {
    Default,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CursorPolicy {
    pub grabbed: bool,
    pub visible: bool,
    pub icon: CursorIcon,
}

impl CursorPolicy {
    pub(super) fn for_screen(screen: AppScreen) -> Self {
        let grabbed = screen.spec().grabs_cursor();
        Self {
            grabbed,
            visible: !grabbed,
            icon: CursorIcon::Default,
        }
    }
}
