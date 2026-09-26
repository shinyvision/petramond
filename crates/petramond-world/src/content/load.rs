//! The ordered, fallible registry build (see the parent module docs).

use std::sync::OnceLock;

use super::{pin, stage, Content, ContentError, ContentErrors, ContentRegistry};
use crate::assets::PackSet;

/// One step of a registry build: a catalog (or derived table) with a name,
/// the stages it reads, and a builder that stores its result in the
/// registry. [`Slot`](super::Slot) is the implementation crates above this
/// one use.
pub trait Stage: Sync {
    /// The stage's name — its catalog file where it has one. Other stages
    /// name it in their [`needs`](Self::needs).
    fn name(&self) -> &'static str;
    /// The stages this one reads; if any of them failed, this one is skipped.
    fn needs(&self) -> &'static [&'static str];
    /// Build into `reg` (the registry being built, which is also the
    /// thread's current registry while the build runs).
    fn build(&self, reg: &ContentRegistry) -> Result<(), String>;
}

/// A stage that fills one of the registry's typed fields.
struct FieldStage {
    name: &'static str,
    needs: &'static [&'static str],
    build: fn(&ContentRegistry) -> Result<(), String>,
}

impl Stage for FieldStage {
    fn name(&self) -> &'static str {
        self.name
    }

    fn needs(&self) -> &'static [&'static str] {
        self.needs
    }

    fn build(&self, reg: &ContentRegistry) -> Result<(), String> {
        (self.build)(reg)
    }
}

fn set<T>(cell: &OnceLock<T>, value: T, what: &str) -> Result<(), String> {
    cell.set(value)
        .map_err(|_| format!("{what} was built twice"))
}

fn build_tiles(reg: &ContentRegistry) -> Result<(), String> {
    set(&reg.tiles, crate::tile::load(reg.packs())?, stage::TILES)
}

fn build_names(reg: &ContentRegistry) -> Result<(), String> {
    set(
        &reg.names,
        crate::registry::load_names(reg.packs())?,
        stage::NAMES,
    )
}

fn build_blocks(reg: &ContentRegistry) -> Result<(), String> {
    set(
        &reg.blocks,
        crate::block::load_registry(reg.packs(), reg.names())?,
        stage::BLOCKS,
    )
}

fn build_block_views(_: &ContentRegistry) -> Result<(), String> {
    crate::block::warm_views();
    Ok(())
}

fn build_items(reg: &ContentRegistry) -> Result<(), String> {
    set(
        &reg.items,
        crate::item::load_tables(reg.packs(), reg.names())?,
        stage::ITEMS,
    )
}

static TILES: FieldStage = FieldStage {
    name: stage::TILES,
    needs: &[],
    build: build_tiles,
};

static NAMES: FieldStage = FieldStage {
    name: stage::NAMES,
    needs: &[],
    build: build_names,
};

static BLOCKS: FieldStage = FieldStage {
    name: stage::BLOCKS,
    needs: &[
        stage::NAMES,
        stage::TILES,
        stage::SOUNDS,
        stage::EFFECTS,
        stage::BIOMES,
        stage::PARTICLE_EMITTERS,
        stage::CONDITIONS,
        stage::ANIMATED_MODELS,
        stage::SHAPES,
        stage::MODELS,
    ],
    build: build_blocks,
};

static BLOCK_VIEWS: FieldStage = FieldStage {
    name: stage::BLOCK_VIEWS,
    needs: &[stage::BLOCKS],
    build: build_block_views,
};

static ITEMS: FieldStage = FieldStage {
    name: stage::ITEMS,
    needs: &[stage::BLOCKS],
    build: build_items,
};

/// The world crate's stages, in build order. Everything a later stage reads
/// comes earlier; `needs` makes the skip-on-failure explicit.
static WORLD_STAGES: &[&dyn Stage] = &[
    &TILES,
    &crate::sound_registry::CATALOG,
    &crate::effect::CATALOG,
    &crate::biome::CATALOG,
    &crate::particle_emitters::TABLES,
    &crate::condition::CATALOG,
    &crate::animated_model::CATALOG,
    &crate::block::CUSTOM_SHAPES,
    &crate::block_model::DEFS,
    &NAMES,
    &BLOCKS,
    &BLOCK_VIEWS,
    &ITEMS,
    &crate::loot::CATALOG,
    &crate::structure::CATALOG,
    &crate::texture_transition::RULES,
    &crate::tile_alpha::TABLE,
    &crate::fluid::medium::MEDIA,
    &crate::section::METRICS,
    &crate::construction::PLACED_BY,
];

/// Builds a [`ContentRegistry`] from a [`PackSet`]: the world crate's stages,
/// then any extension stages in the order given.
pub struct ContentLoader {
    packs: PackSet,
    extra: Vec<&'static dyn Stage>,
}

impl ContentLoader {
    pub fn new(packs: PackSet) -> ContentLoader {
        ContentLoader {
            packs,
            extra: Vec::new(),
        }
    }

    /// Append one extension stage (built after every world stage and every
    /// extension stage added before it).
    pub fn stage(mut self, stage: &'static dyn Stage) -> ContentLoader {
        self.extra.push(stage);
        self
    }

    /// Append several extension stages, in order.
    pub fn stages(mut self, stages: &[&'static dyn Stage]) -> ContentLoader {
        self.extra.extend_from_slice(stages);
        self
    }

    /// Run every stage. `Ok` only when all of them built; otherwise every
    /// error found, with the skipped stages and refused packs.
    pub fn load(self) -> Result<Content, ContentErrors> {
        let refused = self.packs.refused().to_vec();
        // Retained from the start: while the build runs the registry is this
        // thread's current one, so the catalogs' own cross-references (a
        // block row naming a tile, an item naming a sound) resolve against
        // the stages already built — never against another registry.
        let reg: &'static ContentRegistry = Box::leak(Box::new(ContentRegistry::empty(self.packs)));
        let content = Content(reg);
        let mut report = ContentErrors {
            refused,
            ..ContentErrors::default()
        };
        let mut failed: Vec<&'static str> = Vec::new();
        {
            let _pin = pin(content);
            for stage in WORLD_STAGES.iter().chain(self.extra.iter()) {
                if let Some(&need) = stage.needs().iter().find(|n| failed.contains(*n)) {
                    report.skipped.push((stage.name(), need));
                    failed.push(stage.name());
                    continue;
                }
                if let Err(message) = stage.build(reg) {
                    report
                        .errors
                        .extend(
                            message
                                .lines()
                                .filter(|l| !l.trim().is_empty())
                                .map(|line| ContentError {
                                    stage: stage.name(),
                                    message: line.to_owned(),
                                }),
                        );
                    failed.push(stage.name());
                }
            }
        }
        if !failed.is_empty() {
            return Err(report);
        }
        for r in &report.refused {
            log::warn!("content: pack '{}' refused: {}", r.dir_name, r.reason);
        }
        log::info!(
            "content registry #{} built: {} blocks, {} items from {} packs ({} disabled)",
            reg.serial(),
            reg.names().blocks.len(),
            reg.names().items.len(),
            reg.packs().packs().len(),
            reg.packs().disabled().len()
        );
        Ok(content)
    }
}
