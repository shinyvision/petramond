pub(crate) mod capture;

use crate::schematic::share::GhostPlacement;
use crate::schematic::store::Store;
use crate::world::ServerWorld;

pub struct WorldSchematics {
    pub store: Store,
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
