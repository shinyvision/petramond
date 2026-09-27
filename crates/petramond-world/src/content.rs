use std::any::Any;
use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{OnceLock, PoisonError, RwLock};

use crate::assets::{PackRefusal, PackRoots, PackSet};

mod load;

pub use load::{ContentLoader, Stage};

pub mod stage {
    pub const TILES: &str = "textures/atlas.json";
    pub const SOUNDS: &str = "sounds.json";
    pub const EFFECTS: &str = "effects.json";
    pub const BIOMES: &str = "biomes.json";
    pub const PARTICLE_EMITTERS: &str = "particle_emitters.json";
    pub const CONDITIONS: &str = "conditions.json";
    pub const ANIMATED_MODELS: &str = "animated_models.json";
    pub const SHAPES: &str = "shapes.json";
    pub const MODELS: &str = "models.json";
    pub const NAMES: &str = "content names";
    pub const BLOCKS: &str = "blocks.json";
    pub const BLOCK_VIEWS: &str = "block views";
    pub const ITEMS: &str = "items.json";
    pub const LOOT: &str = "loot_tables.json";
    pub const STRUCTURES: &str = "structures.json";
    pub const TEXTURE_TRANSITIONS: &str = "texture_transitions.json";
    pub const TILE_ALPHA: &str = "tile alpha";
    pub const FLUID_MEDIA: &str = "fluid media";
    pub const SECTION_METRICS: &str = "section metrics";
    pub const CONSTRUCTION: &str = "construction placement";
}

#[cfg(test)]
mod tests;

const SLOT_CAPACITY: usize = 96;

static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

pub struct ContentRegistry {
    serial: u64,
    packs: PackSet,
    pub(crate) tiles: OnceLock<crate::tile::TileData>,
    pub(crate) map_rgb: OnceLock<Vec<[u8; 3]>>,
    pub(crate) names: OnceLock<crate::registry::ContentNames>,
    pub(crate) blocks: OnceLock<crate::block::BlockRegistry>,
    pub(crate) block_views: crate::block::BlockViews,
    pub(crate) items: OnceLock<crate::item::ItemTables>,
    pub(crate) variants: crate::item::variant::VariantTable,
    slots: Box<[OnceLock<Box<dyn Any + Send + Sync>>]>,
}

impl ContentRegistry {
    fn empty(packs: PackSet) -> ContentRegistry {
        ContentRegistry {
            serial: NEXT_SERIAL.fetch_add(1, Ordering::Relaxed),
            packs,
            tiles: OnceLock::new(),
            map_rgb: OnceLock::new(),
            names: OnceLock::new(),
            blocks: OnceLock::new(),
            block_views: Default::default(),
            items: OnceLock::new(),
            variants: Default::default(),
            slots: (0..SLOT_CAPACITY).map(|_| OnceLock::new()).collect(),
        }
    }

    pub fn load(packs: PackSet) -> Result<Content, ContentErrors> {
        ContentLoader::new(packs).load()
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn packs(&self) -> &PackSet {
        &self.packs
    }

    #[inline]
    pub fn names(&self) -> &crate::registry::ContentNames {
        self.names.get().unwrap_or_else(|| unbuilt("content names"))
    }

    #[inline]
    pub(crate) fn tiles(&self) -> &crate::tile::TileData {
        self.tiles
            .get()
            .unwrap_or_else(|| unbuilt("textures/atlas.json"))
    }

    #[inline]
    pub(crate) fn blocks(&self) -> &crate::block::BlockRegistry {
        self.blocks.get().unwrap_or_else(|| unbuilt("blocks.json"))
    }

    #[inline]
    pub(crate) fn items(&self) -> &crate::item::ItemTables {
        self.items.get().unwrap_or_else(|| unbuilt("items.json"))
    }

    #[inline]
    pub fn variants(&self) -> &crate::item::variant::VariantTable {
        &self.variants
    }
}

impl fmt::Debug for ContentRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentRegistry")
            .field("serial", &self.serial)
            .field("packs", &self.packs.packs().len())
            .field("disabled", self.packs.disabled())
            .finish()
    }
}

#[cold]
#[inline(never)]
fn unbuilt(stage: &str) -> ! {
    panic!(
        "content stage '{stage}' was read before it was built — a loader stage reads it without \
         declaring it in its `needs`, or the stage failed and its error was ignored"
    )
}

#[derive(Clone, Copy)]
pub struct Content(&'static ContentRegistry);

impl Content {
    pub fn current() -> Content {
        Content(current())
    }

    pub fn registry(self) -> &'static ContentRegistry {
        self.0
    }

    pub fn same(self, other: Content) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl std::ops::Deref for Content {
    type Target = ContentRegistry;

    fn deref(&self) -> &ContentRegistry {
        self.0
    }
}

impl fmt::Debug for Content {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentError {
    pub stage: &'static str,
    pub message: String,
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage, self.message)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ContentErrors {
    pub errors: Vec<ContentError>,
    pub skipped: Vec<(&'static str, &'static str)>,
    pub refused: Vec<PackRefusal>,
}

impl fmt::Display for ContentErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "content failed to load ({} errors)", self.errors.len())?;
        for e in &self.errors {
            writeln!(f, "  {e}")?;
        }
        for (stage, because) in &self.skipped {
            writeln!(f, "  {stage}: not loaded (needs {because}, which failed)")?;
        }
        for r in &self.refused {
            writeln!(f, "  pack '{}' refused: {}", r.dir_name, r.reason)?;
        }
        Ok(())
    }
}

impl std::error::Error for ContentErrors {}

pub struct Slot<T> {
    name: &'static str,
    needs: &'static [&'static str],
    build: fn(&ContentRegistry) -> Result<T, String>,
    index: OnceLock<usize>,
}

impl<T: Send + Sync + 'static> Slot<T> {
    pub const fn new(
        name: &'static str,
        needs: &'static [&'static str],
        build: fn(&ContentRegistry) -> Result<T, String>,
    ) -> Slot<T> {
        Slot {
            name,
            needs,
            build,
            index: OnceLock::new(),
        }
    }

    fn cell<'r>(&self, reg: &'r ContentRegistry) -> &'r OnceLock<Box<dyn Any + Send + Sync>> {
        let index = *self.index.get_or_init(|| {
            let i = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
            assert!(
                i < SLOT_CAPACITY,
                "more than {SLOT_CAPACITY} content slots declared; raise SLOT_CAPACITY"
            );
            i
        });
        &reg.slots[index]
    }

    pub fn get<'r>(&self, reg: &'r ContentRegistry) -> &'r T {
        self.cell(reg)
            .get_or_init(|| match (self.build)(reg) {
                Ok(value) => Box::new(value),
                Err(e) => panic!("{}: {e}", self.name),
            })
            .downcast_ref::<T>()
            .expect("a slot's cell holds its own type")
    }

    pub fn current(&self) -> &'static T {
        self.get(current())
    }
}

impl<T: Send + Sync + 'static> Stage for Slot<T> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn needs(&self) -> &'static [&'static str] {
        self.needs
    }

    fn build(&self, reg: &ContentRegistry) -> Result<(), String> {
        let cell = self.cell(reg);
        if cell.get().is_none() {
            let value = (self.build)(reg)?;
            let _ = cell.set(Box::new(value));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ThreadSlot {
    pinned: Option<&'static ContentRegistry>,
    cached: Option<&'static ContentRegistry>,
    generation: u64,
}

thread_local! {
    static THREAD: Cell<ThreadSlot> = const {
        Cell::new(ThreadSlot { pinned: None, cached: None, generation: 0 })
    };
}

struct Installed {
    registry: Option<&'static ContentRegistry>,
    generation: u64,
}

static INSTALLED: RwLock<Installed> = RwLock::new(Installed {
    registry: None,
    generation: 0,
});
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn current() -> &'static ContentRegistry {
    let slot = THREAD.with(Cell::get);
    if let Some(reg) = slot.pinned {
        return reg;
    }
    match slot.cached {
        Some(reg) if slot.generation == GENERATION.load(Ordering::Acquire) => reg,
        _ => refresh(),
    }
}

pub fn try_current() -> Option<Content> {
    let slot = THREAD.with(Cell::get);
    slot.pinned.or_else(|| read_installed().0).map(Content)
}

fn read_installed() -> (Option<&'static ContentRegistry>, u64) {
    let installed = INSTALLED.read().unwrap_or_else(PoisonError::into_inner);
    (installed.registry, installed.generation)
}

#[cold]
#[inline(never)]
fn refresh() -> &'static ContentRegistry {
    let (registry, generation) = read_installed();
    let registry = match registry {
        Some(reg) => reg,
        None => not_installed(),
    };
    THREAD.with(|t| {
        t.set(ThreadSlot {
            pinned: None,
            cached: Some(registry),
            generation,
        })
    });
    registry
}

#[cfg(not(any(test, feature = "test-support")))]
fn not_installed() -> &'static ContentRegistry {
    panic!(
        "no content registry on this thread: the process must build one and \
         `petramond_world::content::install` it before touching content, or pin one \
         (`content::pin`) on threads that serve another world"
    )
}

#[cfg(any(test, feature = "test-support"))]
fn not_installed() -> &'static ContentRegistry {
    let fallback = test_support::default_content();
    let mut installed = INSTALLED.write().unwrap_or_else(PoisonError::into_inner);
    match installed.registry {
        Some(reg) => reg,
        None => {
            installed.generation += 1;
            installed.registry = Some(fallback.0);
            GENERATION.store(installed.generation, Ordering::Release);
            fallback.0
        }
    }
}

pub fn install(content: Content) {
    let mut installed = INSTALLED.write().unwrap_or_else(PoisonError::into_inner);
    installed.generation += 1;
    installed.registry = Some(content.0);
    GENERATION.store(installed.generation, Ordering::Release);
    log::info!("content registry #{} installed", content.serial());
}

pub fn install_from_env(stages: &[&'static dyn Stage]) -> Result<Content, ContentErrors> {
    let content = ContentLoader::new(PackSet::discover(&PackRoots::from_env()))
        .stages(stages)
        .load()?;
    install(content);
    Ok(content)
}

#[must_use = "the pin lasts only while the guard lives"]
pub struct PinGuard {
    previous: Option<&'static ContentRegistry>,
    _not_send: PhantomData<*const ()>,
}

pub fn pin(content: Content) -> PinGuard {
    let previous = THREAD.with(|t| {
        let mut slot = t.get();
        let previous = slot.pinned.replace(content.0);
        t.set(slot);
        previous
    });
    PinGuard {
        previous,
        _not_send: PhantomData,
    }
}

impl Drop for PinGuard {
    fn drop(&mut self) {
        THREAD.with(|t| {
            let mut slot = t.get();
            slot.pinned = self.previous;
            t.set(slot);
        });
    }
}

pub fn pinned() -> Option<Content> {
    THREAD.with(Cell::get).pinned.map(Content)
}

pub fn activate(content: Content) {
    let replaced = THREAD.with(|t| {
        let mut slot = t.get();
        if slot.pinned.is_none() {
            return false;
        }
        slot.pinned = Some(content.0);
        t.set(slot);
        true
    });
    if !replaced {
        install(content);
    }
}

pub fn for_world(
    disabled: &std::collections::BTreeSet<String>,
    stages: &[&'static dyn Stage],
) -> Result<Content, ContentErrors> {
    static BUILT: std::sync::Mutex<Vec<Content>> = std::sync::Mutex::new(Vec::new());
    let base = Content::current();
    let packs = base.packs().enabled(disabled);
    let same_set = |c: &Content| {
        std::ptr::eq(c.packs().installed().as_ptr(), packs.installed().as_ptr())
            && c.packs().disabled() == packs.disabled()
    };
    if same_set(&base) {
        return Ok(base);
    }
    let mut built = BUILT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hit) = built.iter().copied().find(same_set) {
        return Ok(hit);
    }
    let content = ContentLoader::new(packs).stages(stages).load()?;
    built.push(content);
    Ok(content)
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::path::Path;
    use std::sync::OnceLock;

    use super::{Content, ContentRegistry};
    use crate::assets::{PackRoots, PackSet};

    pub fn default_content() -> Content {
        static DEFAULT: OnceLock<Content> = OnceLock::new();
        *DEFAULT.get_or_init(|| {
            ContentRegistry::load(PackSet::discover(&PackRoots::from_env()))
                .unwrap_or_else(|e| panic!("{e}"))
        })
    }

    pub fn with_mods(mods: &Path) -> Content {
        ContentRegistry::load(PackSet::discover(&PackRoots::with_mods([
            mods.to_path_buf()
        ])))
        .unwrap_or_else(|e| panic!("fixture content failed to load:\n{e}"))
    }
}
