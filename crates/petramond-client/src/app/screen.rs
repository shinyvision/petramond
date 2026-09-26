//! The app's screens and their behaviour table. Every screen's routing,
//! document, HUD, hand, cursor and ESC behaviour is one row of
//! [`AppScreen::spec`]; transitions go through the App's screen funnel
//! (`super::screen_flow`).

use petramond_world::gui_state::GuiKind;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum AppScreen {
    Title,
    WorldSelect,
    /// Per-world mod enablement for the selected world (opened from
    /// world-select; also hosts the relocated Delete World button).
    WorldSettings,
    CreateWorld,
    DeleteWorld,
    /// The "Connect to Server" screen (address + player name).
    /// The connect worker runs while this screen is up.
    ConnectServer,
    /// The refused-join screen listing the server mods this client lacks;
    /// Back returns to [`ConnectServer`](AppScreen::ConnectServer).
    ModsMissing,
    /// The "Disconnected" screen: the session was torn down after a
    /// connection loss / server close; OK returns to the title.
    ConnectionLost,
    /// The Options root (Sound / Controls / Graphics), entered from the title
    /// or pushed over the pause menu (Back pops to whichever is underneath).
    Options,
    OptionsSound,
    /// The controls remap screen; while a binding is armed the App captures
    /// raw input (`App::remap`).
    OptionsControls,
    OptionsGraphics,
    Game,
    /// Chat input overlay: the world keeps ticking and the hotbar HUD stays
    /// visible, but gameplay controls are disabled while text is entered.
    Chat,
    Pause,
    /// A slot-bearing game menu over a live tick: the engine containers
    /// (inventory, crafting table, furnace, chest, furniture workbench) and
    /// mod GUIs, one screen variant for all of them. Carries which registered
    /// kind it draws; the open server session (`ContainerTarget`) speaks the
    /// same kind.
    Menu(GuiKind),
    /// A presentation-only client mod document. It releases the cursor and
    /// receives client-WASM UI events while the replicated world keeps
    /// running; no server menu session exists.
    ClientModGui(GuiKind),
    /// The schematic library opened for a choice a mod asked for: a
    /// client-local document over the running world, with no server menu
    /// session.
    Schematics,
    /// A presentation-only client mod's centered physical-pixel canvas. The
    /// concrete owner/image lives in `Session::client_canvas`; this screen gates
    /// gameplay and releases the cursor without selecting a GUI document.
    ClientCanvas,
    /// The sleep overlay (bed interaction): the simulation KEEPS TICKING under
    /// it — the tick-owned sleep timer drives the fade and the wake — unlike
    /// `Pause`, which freezes the world.
    Sleeping,
    /// The death screen. The simulation keeps ticking (the world does not
    /// freeze around a corpse); ESC cannot close it — only respawn or
    /// save-and-quit leave.
    Dead,
}

/// What kind of frame a screen owns — the first question frame routing asks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ScreenRole {
    /// Captured gameplay: the world owns input and the cursor is grabbed.
    Gameplay,
    /// Chat input over gameplay: the world keeps ticking and the HUD stays
    /// up, but the draft owns the keyboard.
    Chat,
    /// A title-flow, pause or options document. It owns the whole frame and
    /// freezes the simulation behind it — unless the pause is ineffective
    /// (a multiplayer pause), which the App decides.
    Shell,
    /// A gameplay overlay (sleep fade, death): its document owns input like a
    /// shell screen, but the simulation keeps ticking — the sleep timer and
    /// the respawn are tick-owned.
    Overlay,
    /// A slot-bearing game menu with a server menu session behind it.
    GameMenu,
    /// A client-local document over the running world (a client mod's GUI,
    /// the schematic library): no server menu session.
    ClientDoc,
    /// A client mod's physical-pixel canvas.
    Canvas,
}

impl ScreenRole {
    /// Whether the simulation ticks behind the screen. A shell screen over a
    /// live game freezes it (the multiplayer pause exception is the App's).
    #[inline]
    pub(super) fn runs_sim(self) -> bool {
        self != ScreenRole::Shell
    }
}

/// What the close-screen control (ESC) does on a screen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Escape {
    /// Nothing to close: the control is not consumed (the title screen).
    Unhandled,
    /// Swallowed: only the screen's own buttons leave (the death screen).
    Blocked,
    /// Back to the screen underneath when one was pushed, else to this one.
    Back(AppScreen),
    /// An options category: an armed control remap is disarmed first;
    /// otherwise [`Escape::Back`].
    CancelRemapOrBack(AppScreen),
    /// Cancel the connect worker and return to the title.
    CancelConnect,
    /// Back to the connect screen with the refused attempt's fields intact.
    ReopenConnect,
    /// Close the server menu session and return to gameplay.
    CloseMenu,
    /// Ask the tick to wake the player and drop the sleep overlay.
    CancelSleep,
    /// Cancel an active world tool, or else open the pause menu.
    PauseGame,
    /// Close the pause menu and resume the game.
    ResumeGame,
}

impl Escape {
    /// The screen a Back leads to when nothing was pushed underneath.
    #[inline]
    pub(super) fn back_target(self) -> Option<AppScreen> {
        match self {
            Escape::Back(parent) | Escape::CancelRemapOrBack(parent) => Some(parent),
            _ => None,
        }
    }
}

/// Whether the first-person hand draws over a screen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum HandPolicy {
    /// Shown (third person shows the whole body instead).
    Shown,
    Hidden,
    /// Only while the bed-interaction jab plays out over the sleep fade.
    SleepFade,
}

/// Everything that differs between screens, described in one place: frame
/// routing, the GUI document, the HUD, the hand, the cursor and ESC all read
/// it instead of matching on [`AppScreen`] themselves.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ScreenSpec {
    pub(super) role: ScreenRole,
    /// The GUI document that backs the screen, if any (it only draws when
    /// the document is loaded). The hotbar HUD is not a screen document: it
    /// draws under `hud` screens.
    pub(super) doc: Option<GuiKind>,
    /// The hotbar HUD and chat draw over the world.
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

    /// Whether the cursor is captured for gameplay (hidden and grabbed).
    #[inline]
    pub(super) fn grabs_cursor(&self) -> bool {
        self.role == ScreenRole::Gameplay
    }
}

impl AppScreen {
    /// The screen's behaviour table.
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
            S::ModsMissing => shell(GuiKind::ModsMissing, E::ReopenConnect),
            S::ConnectionLost => shell(GuiKind::ConnectionLost, E::Back(S::Title)),
            S::Options => shell(GuiKind::Options, E::Back(S::Title)),
            S::OptionsSound => shell(GuiKind::OptionsSound, E::CancelRemapOrBack(S::Options)),
            S::OptionsControls => {
                shell(GuiKind::OptionsControls, E::CancelRemapOrBack(S::Options))
            }
            S::OptionsGraphics => {
                shell(GuiKind::OptionsGraphics, E::CancelRemapOrBack(S::Options))
            }
            S::Pause => shell(GuiKind::Pause, E::ResumeGame).with_hand(HandPolicy::Hidden),
            S::Game => Spec::new(R::Gameplay, None, E::PauseGame).with_hud(),
            S::Chat => Spec::new(R::Chat, None, E::Back(S::Game)).with_hud(),
            S::Menu(kind) => Spec::new(R::GameMenu, Some(kind), E::CloseMenu),
            S::ClientModGui(kind) => Spec::new(R::ClientDoc, Some(kind), E::Back(S::Game)),
            S::Schematics => Spec::new(R::ClientDoc, Some(GuiKind::Schematics), E::Back(S::Game)),
            S::ClientCanvas => Spec::new(R::Canvas, None, E::Back(S::Game)),
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
    pub(super) fn shell_open(self) -> bool {
        self.role() == ScreenRole::Shell
    }

    /// One of the Options screens (root or a category) is open.
    #[inline]
    pub(super) fn options_open(self) -> bool {
        matches!(
            self,
            AppScreen::Options
                | AppScreen::OptionsSound
                | AppScreen::OptionsControls
                | AppScreen::OptionsGraphics
        )
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn inventory_open(self) -> bool {
        self == AppScreen::Menu(GuiKind::Inventory)
    }

    /// A gameplay overlay screen is up (sleep fade / death).
    #[inline]
    pub(super) fn overlay_open(self) -> bool {
        self.role() == ScreenRole::Overlay
    }

    /// Any slot-based menu (container or mod GUI) is open — drives click
    /// routing and whether the panel UI is drawn.
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

    /// Which GUI the render snapshot names for this screen: the open menu's
    /// or overlay's document, `Hotbar` for the HUD screens, `Other` for shell
    /// screens and the canvas (their documents, when loaded, override it).
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

/// Known polish gap: document text inputs don't request a Text (I-beam)
/// cursor yet, so Default is currently the only icon.
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
