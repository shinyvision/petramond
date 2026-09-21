use crate::world::World;
use petramond_math::math::IVec3;
use petramond_world::container::Container;
use petramond_world::gui_state::GuiKind;
use serde::{Deserialize, Serialize};

/// What a GUI session is anchored on: the thing whose slot storage backs the
/// document's `container` slots, and the identity every `gui_click` carries.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MenuAnchor {
    /// A world cell (canonicalized to the block's container anchor).
    Block(IVec3),
    /// A live mob, by stable id; its carried slots are the storage. The
    /// session ends when the mob leaves the world.
    Mob(u64),
}

impl From<IVec3> for MenuAnchor {
    fn from(pos: IVec3) -> Self {
        MenuAnchor::Block(pos)
    }
}

impl MenuAnchor {
    #[inline]
    pub fn block(self) -> Option<IVec3> {
        match self {
            MenuAnchor::Block(pos) => Some(pos),
            MenuAnchor::Mob(_) => None,
        }
    }

    /// Whether the anchored thing is still there to hold a session open. A
    /// block anchor always is (a broken block's session just shows no slots);
    /// a mob must be in the world and alive.
    pub fn present(self, world: &World) -> bool {
        match self {
            MenuAnchor::Block(_) => true,
            MenuAnchor::Mob(id) => live_mob(world, id).is_some(),
        }
    }

    /// The slot storage behind the anchor.
    pub fn container(self, world: &World) -> Option<&Container> {
        match self {
            MenuAnchor::Block(pos) => world.container_at(pos),
            MenuAnchor::Mob(id) => {
                live_mob(world, id).map(|i| world.mobs().instances()[i].container())
            }
        }
    }

    /// Mutate the slot storage behind the anchor and record the change: a
    /// block container persists with its section, so the section is marked; a
    /// mob's slots ride its own record. `None` = nothing is stored there.
    pub fn edit_container<R>(
        self,
        world: &mut World,
        edit: impl FnOnce(&mut Container) -> R,
    ) -> Option<R> {
        match self {
            MenuAnchor::Block(pos) => {
                let out = world.container_at_mut(pos).map(edit);
                world.mark_chunk_modified(pos);
                out
            }
            MenuAnchor::Mob(id) => {
                let index = live_mob(world, id)?;
                world.mobs_mut().container_mut(index).map(edit)
            }
        }
    }
}

fn live_mob(world: &World, id: u64) -> Option<usize> {
    let index = world.mobs().index_of_id(id)?;
    (!world.mobs().instances()[index].is_dead()).then_some(index)
}

/// What the open GUI is acting on — named for the thing being edited, not for the
/// screen. The app's `AppScreen` decides which screen is up; this decides which
/// block-entity or transient station session that screen reads and mutates.
///
/// ONE shape for every kind: engine containers and mod GUIs ride the same
/// `kind + anchor` session identity. What the session means per kind (a
/// block-entity container, a transient crafting station, machine gauges) is
/// resolved by the per-kind lookups beside the menu, never by growing this
/// enum.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ContainerTarget {
    /// No container GUI is editing anything (gameplay, or a screen that owns no
    /// block-entity yet).
    #[default]
    None,
    /// The GUI session for `kind`. `anchor` is what the session was opened
    /// on: the block (canonicalized to the container anchor for mod kinds) or
    /// the mob; `None` for transient stations (inventory/table crafting, the
    /// furniture workbench) and unanchored `GuiOpen`s. It rides the session
    /// into every `gui_click` dispatch.
    Gui {
        kind: GuiKind,
        anchor: Option<MenuAnchor>,
    },
}

impl ContainerTarget {
    /// The open session's GUI kind, if any.
    #[inline]
    pub fn kind(self) -> Option<GuiKind> {
        match self {
            ContainerTarget::None => None,
            ContainerTarget::Gui { kind, .. } => Some(kind),
        }
    }

    /// The open session's anchor, if any.
    #[inline]
    pub fn anchor(self) -> Option<MenuAnchor> {
        match self {
            ContainerTarget::None => None,
            ContainerTarget::Gui { anchor, .. } => anchor,
        }
    }

    /// Whether kind `kind`'s session edits the `Container` stored on its
    /// anchor (the chest/furnace block entities, and mod GUIs opened on a
    /// block or a mob). Transient station kinds keep their stacks on the menu
    /// itself.
    #[inline]
    pub fn kind_anchor_backed(kind: GuiKind) -> bool {
        kind == GuiKind::Chest || kind == GuiKind::Furnace || kind.is_registered()
    }
}
