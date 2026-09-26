//! Every pack id this mod names, declared once and checked against the
//! shipped JSON by the test below (see [`mod_sdk::pack_keys!`]).

mod_sdk::pack_keys! {
    /// The waypoint editor documents.
    pub(crate) CREATE_WAYPOINT_GUI: GuiKind = "minimap:create_waypoint";
    pub(crate) EDIT_WAYPOINT_GUI: GuiKind = "minimap:edit_waypoint";
    /// The name field's bound text, shared by both editors.
    pub(crate) WAYPOINT_NAME: GuiState = "minimap:waypoint_name";

    /// Editor widgets. Both documents carry name/save/cancel (the edit
    /// document's copies are asserted by the test below); only the edit
    /// document has delete.
    pub(crate) WIDGET_NAME: Widget(CREATE_WAYPOINT_GUI) = "name";
    pub(crate) WIDGET_SAVE: Widget(CREATE_WAYPOINT_GUI) = "save";
    pub(crate) WIDGET_CANCEL: Widget(CREATE_WAYPOINT_GUI) = "cancel";
    pub(crate) WIDGET_DELETE: Widget(EDIT_WAYPOINT_GUI) = "delete";
}

#[cfg(test)]
mod tests {
    use mod_sdk::{PackKey, PackKeyKind};

    use super::*;

    /// The shared widgets must exist in the edit document too — the click
    /// router matches them by id alone, whichever editor is open.
    const EDIT_SHARED: &[PackKey] = &[
        PackKey {
            kind: PackKeyKind::Widget(EDIT_WAYPOINT_GUI),
            key: WIDGET_NAME,
        },
        PackKey {
            kind: PackKeyKind::Widget(EDIT_WAYPOINT_GUI),
            key: WIDGET_SAVE,
        },
        PackKey {
            kind: PackKeyKind::Widget(EDIT_WAYPOINT_GUI),
            key: WIDGET_CANCEL,
        },
    ];

    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[PACK_KEYS, EDIT_SHARED]);
    }
}
