use std::sync::OnceLock;

use super::{pin, stage, Content, ContentError, ContentErrors, ContentRegistry};
use crate::assets::PackSet;

pub trait Stage: Sync {
    fn name(&self) -> &'static str;
    fn needs(&self) -> &'static [&'static str];
    fn build(&self, reg: &ContentRegistry) -> Result<(), String>;
}

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

    pub fn stage(mut self, stage: &'static dyn Stage) -> ContentLoader {
        self.extra.push(stage);
        self
    }

    pub fn stages(mut self, stages: &[&'static dyn Stage]) -> ContentLoader {
        self.extra.extend_from_slice(stages);
        self
    }

    pub fn load(self) -> Result<Content, ContentErrors> {
        let refused = self.packs.refused().to_vec();
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
