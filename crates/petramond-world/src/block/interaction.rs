#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockInteraction {
    None,
    OpenGui(crate::gui_state::GuiKind),
    ToggleDoor,
    ToggleTrapdoor,
    Sleep,
}

pub fn builtin_claims_click(block: crate::block::Block, sneaking: bool) -> bool {
    !sneaking && block.interaction() != BlockInteraction::None
}
