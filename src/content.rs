//! The engine's content bootstrap: the extension stages the engine layers over
//! the world crate's registry build (worldgen's catalogs, the mob catalog),
//! and the one-call install every engine binary runs before touching content.

use std::collections::BTreeSet;

use petramond_world::content::{Content, ContentErrors, Stage};

/// Every catalog stage the engine adds to a registry build, in order.
pub fn stages() -> Vec<&'static dyn Stage> {
    let mut stages: Vec<&'static dyn Stage> = petramond_worldgen::data::content_stages().to_vec();
    stages.push(&crate::mob::CATALOG);
    stages
}

/// Build the launch environment's registry with the engine's stages plus
/// `extra` (a client adds its presentation catalogs) and install it for the
/// process. On error nothing is installed and every problem comes back.
pub fn install_from_env(extra: &[&'static dyn Stage]) -> Result<Content, ContentErrors> {
    let mut all = stages();
    all.extend_from_slice(extra);
    petramond_world::content::install_from_env(&all)
}

/// The registry for a world that switched `disabled` off, with the engine's
/// stages (see `petramond_world::content::for_world`).
pub fn for_world(disabled: &BTreeSet<String>) -> Result<Content, ContentErrors> {
    petramond_world::content::for_world(disabled, &stages())
}
