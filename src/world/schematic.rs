pub(crate) mod capture;

use crate::schematic::share::GhostPlacement;
use crate::schematic::store::Store;
use crate::world::ServerWorld;

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
    pub viewers: Vec<crate::world::session::PlayerId>,
}

impl Default for WorldSchematics {
    fn default() -> Self {
        Self {
            store: Store::new(None),
            ghosts: Default::default(),
        }
    }
}

impl ServerWorld {
    pub fn schematics(&self) -> &WorldSchematics {
        &self.side.schematics
    }

    pub fn schematics_mut(&mut self) -> &mut WorldSchematics {
        &mut self.side.schematics
    }
}
