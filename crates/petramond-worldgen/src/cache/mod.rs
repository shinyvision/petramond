mod context;
pub(crate) mod local;
pub(crate) mod memo;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use petramond_world::section::BlockCube;

pub use context::GenContext;
pub(crate) use memo::{Memo, MemoSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scaling {
    Frontier,
    Fixed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CacheBudget {
    scale: f64,
}

impl CacheBudget {
    pub const REFERENCE: Self = Self { scale: 1.0 };
    pub const REFERENCE_WORKERS: usize = 8;
    const SCALE_RANGE: (f64, f64) = (0.5, 4.0);

    pub fn for_world(view_distance: i32, workers: usize) -> Self {
        let reference_view = f64::from(petramond_world::world::load_targets::RENDER_DIST);
        let view = f64::from(view_distance.max(1)) / reference_view;
        let threads = workers.max(1) as f64 / Self::REFERENCE_WORKERS as f64;
        Self {
            scale: view
                .max(threads)
                .clamp(Self::SCALE_RANGE.0, Self::SCALE_RANGE.1),
        }
    }

    pub(crate) fn capacity(self, base: usize, scaling: Scaling) -> usize {
        match scaling {
            Scaling::Fixed => base.next_power_of_two(),
            Scaling::Frontier => ((base as f64 * self.scale).round() as usize).next_power_of_two(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoStats {
    pub name: &'static str,
    pub shared: bool,
    pub capacity: usize,
    pub entries: usize,
    pub bytes: u64,
    pub hits: u64,
    pub misses: u64,
}

impl MemoStats {
    pub fn hit_rate(&self) -> Option<f64> {
        let lookups = self.hits + self.misses;
        (lookups > 0).then(|| self.hits as f64 / lookups as f64)
    }
}

macro_rules! memo_group {
    (
        $(#[$meta:meta])*
        $vis:vis struct $group:ident {
            $(
                $(#[$doc:meta])*
                $field:ident: $key:ty => $value:ty =
                    ($name:literal, $capacity:expr, $scaling:ident, $weigh:expr),
            )*
        }
    ) => {
        $(#[$meta])*
        $vis struct $group {
            $( $(#[$doc])* pub(super) $field: $crate::cache::Memo<$key, $value>, )*
        }

        impl $group {
            pub(crate) fn new(budget: $crate::cache::CacheBudget) -> Self {
                Self {
                    $(
                        $field: $crate::cache::Memo::new(
                            $crate::cache::MemoSpec {
                                name: $name,
                                capacity: $capacity,
                                scaling: $crate::cache::Scaling::$scaling,
                                weigh: $weigh,
                            },
                            budget,
                        ),
                    )*
                }
            }

            pub(crate) fn stats(&self) -> Vec<$crate::cache::MemoStats> {
                vec![$( self.$field.stats(), )*]
            }

            #[allow(dead_code)]
            pub(crate) fn clear(&self) {
                $( self.$field.clear(); )*
            }
        }
    };
}
pub(crate) use memo_group;

pub(crate) fn inline<V>(_: &V) -> usize {
    0
}

pub(crate) fn pointee<T>(_: &Arc<T>) -> usize {
    std::mem::size_of::<T>()
}

pub(crate) fn slice<T>(v: &Arc<[T]>) -> usize {
    std::mem::size_of_val(&**v)
}

fn cube_heap(cube: &BlockCube) -> usize {
    cube.heap().1 as usize
}

memo_group! {
    pub(crate) struct TerrainCaches {
        section_cubes: crate::section_memo::Key => BlockCube =
            ("terrain.section_cubes", 8192, Frontier, cube_heap),
        section_spaces: crate::section_memo::Key => Arc<crate::section_memo::SpaceMask> =
            ("terrain.section_spaces", 16_384, Frontier, pointee),
        columns: crate::density::columns::Key => Arc<crate::density::columns::Tile> =
            ("terrain.columns", 2048, Frontier, pointee),
        surface_tiles: crate::feature::TileKey => Arc<crate::feature::RegionTile> =
            ("terrain.surface_tiles", 2048, Frontier, pointee),
        underground_boxes: crate::UndergroundBoxKey => Arc<[u8]> =
            ("terrain.underground_boxes", 4096, Frontier, slice),
        ore_veins: crate::feature::scatter::VeinKey => Arc<crate::feature::scatter::ColumnVeins> =
            ("terrain.ore_veins", 2048, Frontier, |v: &Arc<crate::feature::scatter::ColumnVeins>| v.heap_bytes()),
        placed_features: crate::feature::placement::PlacedKey =>
            Arc<crate::feature::placement::PlacedFeature> =
            ("terrain.placed_features", 256, Frontier, crate::feature::placement::placed_heap),
    }
}

pub struct GenCaches {
    budget: CacheBudget,
    pub(crate) terrain: TerrainCaches,
    pub(crate) caves: crate::noise::cave_field::CaveCaches,
}

impl GenCaches {
    pub fn new(budget: CacheBudget) -> Self {
        Self {
            budget,
            terrain: TerrainCaches::new(budget),
            caves: crate::noise::cave_field::CaveCaches::new(budget),
        }
    }

    pub fn budget(&self) -> CacheBudget {
        self.budget
    }

    pub fn report(&self) -> Vec<MemoStats> {
        let mut out = self.terrain.stats();
        out.extend(self.caves.stats());
        out.extend(local::stats());
        out.extend(process().stats());
        out
    }

    pub fn resident_bytes(&self) -> u64 {
        self.report().iter().map(|m| m.bytes).sum()
    }

    pub fn clear(&self) {
        self.terrain.clear();
        self.caves.clear();
        local::clear_all();
    }
}

memo_group! {
    pub(crate) struct ProcessCaches {
        noises: crate::formula::NoiseKey => Arc<crate::density::noise::ReferenceDoublePerlin> =
            ("process.noises", 256, Fixed, pointee),
    }
}

pub(crate) fn process() -> &'static ProcessCaches {
    static PROCESS: LazyLock<ProcessCaches> =
        LazyLock::new(|| ProcessCaches::new(CacheBudget::REFERENCE));
    &PROCESS
}

static INSTALLED: RwLock<Option<Arc<GenCaches>>> = RwLock::new(None);
static INSTALLED_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn install(caches: Arc<GenCaches>) {
    *INSTALLED.write().unwrap_or_else(PoisonError::into_inner) = Some(caches);
    INSTALLED_EPOCH.fetch_add(1, Ordering::AcqRel);
}

pub fn installed() -> Arc<GenCaches> {
    if let Some(caches) = INSTALLED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        return Arc::clone(caches);
    }
    let mut slot = INSTALLED.write().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(slot.get_or_insert_with(|| Arc::new(GenCaches::new(CacheBudget::REFERENCE))))
}

pub fn installed_epoch() -> u64 {
    INSTALLED_EPOCH.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_scales_frontier_memos_within_bounds() {
        let reference_view = petramond_world::world::load_targets::RENDER_DIST;
        assert_eq!(
            CacheBudget::for_world(reference_view, CacheBudget::REFERENCE_WORKERS),
            CacheBudget::REFERENCE
        );
        let far = CacheBudget::for_world(reference_view * 2, 4);
        assert_eq!(far.capacity(8192, Scaling::Frontier), 16_384);
        assert_eq!(far.capacity(512, Scaling::Fixed), 512);
        let many = CacheBudget::for_world(8, CacheBudget::REFERENCE_WORKERS * 3);
        assert_eq!(many.capacity(1000, Scaling::Frontier), 4096);
        let tiny = CacheBudget::for_world(2, 1);
        assert_eq!(
            tiny.capacity(8192, Scaling::Frontier),
            4096,
            "never below half"
        );
        let huge = CacheBudget::for_world(reference_view * 100, 1);
        assert_eq!(
            huge.capacity(8192, Scaling::Frontier),
            32_768,
            "never above four times"
        );
    }

    #[test]
    fn a_world_report_lists_every_memo_and_clears() {
        let caches = Arc::new(GenCaches::new(CacheBudget::REFERENCE));
        let report = caches.report();
        let mut names: Vec<_> = report.iter().map(|m| m.name).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(unique, names.len(), "duplicate memo names in {names:?}");
        assert!(report
            .iter()
            .all(|m| m.capacity.is_power_of_two() || !m.shared));

        let generator =
            crate::driver::ChunkGenerator::with_caches(0xCAC4_E001, None, Arc::clone(&caches));
        let col = generator.generate_column_gen(0, 0);
        let (lo, hi) = col.surf_range();
        for cy in [lo.div_euclid(16), hi.div_euclid(16)] {
            generator.generate_section(petramond_world::chunk::SectionPos::new(0, cy, 0), &col);
        }
        let shared = |caches: &GenCaches| -> usize {
            caches
                .report()
                .iter()
                .filter(|m| m.shared)
                .map(|m| m.entries)
                .sum()
        };
        assert!(shared(&caches) > 0, "generation filled no memo");
        let tiles = caches.terrain.surface_tiles.stats();
        assert!(tiles.entries > 0 && tiles.misses > 0);
        assert!(caches.resident_bytes() > 0);
        caches.clear();
        let after: usize = caches
            .terrain
            .stats()
            .into_iter()
            .chain(caches.caves.stats())
            .map(|m| m.entries)
            .sum();
        assert_eq!(after, 0, "a cleared world keeps memoized values");
    }
}
