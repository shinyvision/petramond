//! App-side state that lives exactly as long as one game session: the HUD's
//! hotbar notice, the creative menu's and schematic library's form state, and
//! the host's Open to LAN status. Starting or leaving a session replaces the
//! whole value, so nothing here can leak from one world into the next.

use super::creative::CreativeMenu;
use super::hotbar_notice::HotbarNotice;
use super::schematic_library::LibraryForm;

#[derive(Default)]
pub(super) struct SessionUi {
    pub(super) hotbar_notice: HotbarNotice,
    pub(super) creative_menu: CreativeMenu,
    pub(super) library_form: LibraryForm,
    /// The port the running HOST session is open to LAN on (`None` = not
    /// open). Drives the pause menu's Open to LAN button/label.
    pub(super) lan_port: Option<u16>,
    /// The last Open to LAN failure, shown inline on the pause menu; cleared
    /// when the pause screen closes.
    pub(super) lan_error: Option<String>,
}
