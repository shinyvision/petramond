pub(crate) mod capture;

use super::World;
use crate::schematic::share::GhostPlacement;
use crate::schematic::store::Store;

/// A world's schematic assets and the ghosts anchored in it.
pub struct WorldSchematics {
    pub store: Store,
    /// Retained anchored ghosts by their namespaced key: what each shows and
    /// who sees it (empty = everyone). Presentation, never persisted — the
    /// owner re-sets them.
    pub ghosts: std::collections::BTreeMap<String, Ghost>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ghost {
    pub placement: GhostPlacement,
    pub viewers: Vec<crate::player::PlayerId>,
}

impl Default for WorldSchematics {
    fn default() -> Self {
        Self {
            store: Store::new(None),
            ghosts: Default::default(),
        }
    }
}

impl World {
    pub fn schematics(&self) -> &WorldSchematics {
        &self.schematics
    }

    pub fn schematics_mut(&mut self) -> &mut WorldSchematics {
        &mut self.schematics
    }
}
