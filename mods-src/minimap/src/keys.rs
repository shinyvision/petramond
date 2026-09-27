mod_sdk::pack_keys! {
    pub(crate) CREATE_WAYPOINT_GUI: GuiKind = "minimap:create_waypoint";
    pub(crate) EDIT_WAYPOINT_GUI: GuiKind = "minimap:edit_waypoint";
    pub(crate) WAYPOINT_NAME: GuiState = "minimap:waypoint_name";

    pub(crate) WIDGET_NAME: Widget(CREATE_WAYPOINT_GUI) = "name";
    pub(crate) WIDGET_SAVE: Widget(CREATE_WAYPOINT_GUI) = "save";
    pub(crate) WIDGET_CANCEL: Widget(CREATE_WAYPOINT_GUI) = "cancel";
    pub(crate) WIDGET_DELETE: Widget(EDIT_WAYPOINT_GUI) = "delete";
}

#[cfg(test)]
mod tests {
    use mod_sdk::{PackKey, PackKeyKind};

    use super::*;

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
