use crate::world::ServerWorld;
use petramond_math::math::IVec3;
use petramond_world::container::Container;
use petramond_world::gui_state::GuiKind;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MenuAnchor {
    Block(IVec3),
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

    pub fn present(self, world: &ServerWorld) -> bool {
        match self {
            MenuAnchor::Block(_) => true,
            MenuAnchor::Mob(id) => world.mobs().live(id).is_some(),
        }
    }

    pub fn container(self, world: &ServerWorld) -> Option<&Container> {
        match self {
            MenuAnchor::Block(pos) => world.container_at(pos),
            MenuAnchor::Mob(id) => world.mobs().live(id).map(|m| m.container()),
        }
    }

    pub fn edit_container<R>(
        self,
        world: &mut ServerWorld,
        edit: impl FnOnce(&mut Container) -> R,
    ) -> Option<R> {
        match self {
            MenuAnchor::Block(pos) => {
                let out = world.container_at_mut(pos).map(edit);
                world.mark_chunk_modified(pos);
                out
            }
            MenuAnchor::Mob(id) => {
                world.mobs().live(id)?;
                world.mobs_mut().container_mut(id).map(edit)
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ContainerTarget {
    #[default]
    None,
    Gui {
        kind: GuiKind,
        anchor: Option<MenuAnchor>,
    },
}

impl ContainerTarget {
    #[inline]
    pub fn kind(self) -> Option<GuiKind> {
        match self {
            ContainerTarget::None => None,
            ContainerTarget::Gui { kind, .. } => Some(kind),
        }
    }

    #[inline]
    pub fn anchor(self) -> Option<MenuAnchor> {
        match self {
            ContainerTarget::None => None,
            ContainerTarget::Gui { anchor, .. } => anchor,
        }
    }

    #[inline]
    pub fn kind_anchor_backed(kind: GuiKind) -> bool {
        kind == GuiKind::Chest || kind == GuiKind::Furnace || kind.is_registered()
    }
}
