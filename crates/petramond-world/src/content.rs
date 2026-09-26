//! The content registry: every pack-extensible catalog as ONE explicit,
//! immutable value, built by a fallible loader from a [`PackSet`].
//!
//! # Building
//!
//! [`ContentLoader`] builds a [`ContentRegistry`] in a fixed stage order
//! (tiles before the catalogs that name tiles, the shared block/item name
//! tables before either definition table, ...; see `load::WORLD_STAGES`).
//! Every stage runs eagerly; a stage that fails records its errors — one
//! [`ContentError`] per bad row where the catalog reports rows — and the
//! stages that declared a dependency on it are skipped rather than run
//! against missing data. The loader returns every error at once
//! ([`ContentErrors`]), so a bad pack produces a load report, never a panic
//! on whichever worker thread happened to touch a catalog first.
//!
//! Crates above this one attach their own catalogs through [`Slot`]s: a
//! `static` slot names its builder, the crate's loader passes it to
//! [`ContentLoader::stage`], and it builds in order with the world stages.
//!
//! # Using
//!
//! A built registry is a [`Content`] handle — `Copy`, and cheap to hand to a
//! world, a server, a client or a job. Registries are immutable and are
//! retained for the rest of the process once built: every definition row is
//! handed out as `&'static`, which is what keeps the per-cell hot paths an
//! index into a table instead of a lock or a refcount. Reloading packs means
//! building a new registry and making it current; the old one stays valid
//! for anything still holding its rows.
//!
//! # The current registry
//!
//! The typed accessors all over the codebase (`Block::is_opaque`,
//! `ItemType::def`, `sound_registry::by_name`, ...) resolve through
//! [`current`]: the registry PINNED on this thread ([`pin`]; worker pools pin
//! the submitter's registry around every job), else the one INSTALLED for the
//! process ([`install`]). Binaries install a registry at startup, before any
//! content is touched; a thread with neither is a bootstrap bug and panics
//! with a clear message. Test builds instead fall back to one shared default
//! built from [`PackRoots::from_env`], and a test that needs its own content
//! builds a registry and pins it.

use std::any::Any;
use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{OnceLock, PoisonError, RwLock};

use crate::assets::{PackRefusal, PackRoots, PackSet};

mod load;

pub use load::{ContentLoader, Stage};

/// The world crate's stage names — what a [`Slot`]'s `needs` lists to build
/// after (and be skipped with) one of them.
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
    /// The shared block + item name tables (keys of both catalogs).
    pub const NAMES: &str = "content names";
    pub const BLOCKS: &str = "blocks.json";
    /// The dense per-id tables derived from the block rows.
    pub const BLOCK_VIEWS: &str = "block views";
    pub const ITEMS: &str = "items.json";
    pub const LOOT: &str = "loot_tables.json";
    pub const STRUCTURES: &str = "structures.json";
    pub const TEXTURE_TRANSITIONS: &str = "texture_transitions.json";
    /// Per-tile cutout alpha masks (decoded from the tile textures).
    pub const TILE_ALPHA: &str = "tile alpha";
    pub const FLUID_MEDIA: &str = "fluid media";
    pub const SECTION_METRICS: &str = "section metrics";
    pub const CONSTRUCTION: &str = "construction placement";
}

#[cfg(test)]
mod tests;

/// Most [`Slot`]s one process may declare. Slots are `static`s in code, so
/// the count is fixed at compile time; this only sizes the per-registry
/// table.
const SLOT_CAPACITY: usize = 96;

static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// Every pack-extensible catalog of one pack set (see the module docs).
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

    /// Build a registry over `packs` with the world crate's stages only (no
    /// extension slots) — [`ContentLoader::new`]`(packs).load()`.
    pub fn load(packs: PackSet) -> Result<Content, ContentErrors> {
        ContentLoader::new(packs).load()
    }

    /// A process-unique number for this registry (diagnostics; derived caches
    /// outside the registry key on it).
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// The pack set this registry was built from.
    pub fn packs(&self) -> &PackSet {
        &self.packs
    }

    /// The shared block + item name tables.
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

    /// This registry's item instance-data table (variant ids are only
    /// meaningful against the registry that minted them).
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

/// A built, retained registry (see the module docs). `Copy`; derefs to the
/// registry.
#[derive(Clone, Copy)]
pub struct Content(&'static ContentRegistry);

impl Content {
    /// The registry [`current`] resolves to on this thread.
    pub fn current() -> Content {
        Content(current())
    }

    /// The registry, for the rest of the process.
    pub fn registry(self) -> &'static ContentRegistry {
        self.0
    }

    /// Whether both handles name the same registry.
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

/// One content problem, attributed to the catalog stage that found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentError {
    /// The stage (catalog file) that failed, e.g. `blocks.json`.
    pub stage: &'static str,
    pub message: String,
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage, self.message)
    }
}

/// Everything a failed [`ContentLoader::load`] found: every error of every
/// stage that ran, the stages skipped because something they need failed,
/// and the packs discovery had already refused.
#[derive(Clone, Debug, Default)]
pub struct ContentErrors {
    pub errors: Vec<ContentError>,
    /// `(stage, the failed stage it needed)`.
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

/// A catalog a crate above this one keeps in every registry — a `static`
/// with a name, the stages it needs, and a builder over the registry being
/// built. Pass it to [`ContentLoader::stage`] so it builds (and reports its
/// errors) with the rest; a slot nobody registered builds on first use
/// instead, which is reserved for INFALLIBLE derived views (a failing builder
/// there panics with its message).
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

    /// This slot's value in `reg`.
    pub fn get<'r>(&self, reg: &'r ContentRegistry) -> &'r T {
        self.cell(reg)
            .get_or_init(|| match (self.build)(reg) {
                Ok(value) => Box::new(value),
                Err(e) => panic!("{}: {e}", self.name),
            })
            .downcast_ref::<T>()
            .expect("a slot's cell holds its own type")
    }

    /// This slot's value in the [`current`] registry.
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
            // A dependent stage may have forced the slot in the meantime.
            let _ = cell.set(Box::new(value));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The current registry: per-thread pin, then the process install.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct ThreadSlot {
    pinned: Option<&'static ContentRegistry>,
    /// The install this thread last resolved, and its generation.
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
/// Mirrors `INSTALLED.generation` so the hot path checks it without a lock.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The registry content accessors read on this thread: the pinned one, else
/// the process install (see the module docs). One thread-local read and one
/// atomic load on the hot path.
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

/// [`current`], or `None` on a thread with neither a pin nor an install —
/// never falls back or panics.
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

/// Make `content` the process registry for every thread without a pin.
/// Threads pick the change up on their next content access; rows of the
/// previous registry stay valid (registries are retained).
pub fn install(content: Content) {
    let mut installed = INSTALLED.write().unwrap_or_else(PoisonError::into_inner);
    installed.generation += 1;
    installed.registry = Some(content.0);
    GENERATION.store(installed.generation, Ordering::Release);
    log::info!("content registry #{} installed", content.serial());
}

/// Build the environment's registry ([`PackRoots::from_env`]) with `stages`
/// as the extra (extension) stages and install it — the binaries' startup
/// step. On error nothing is installed and the full report comes back.
pub fn install_from_env(stages: &[&'static dyn Stage]) -> Result<Content, ContentErrors> {
    let content = ContentLoader::new(PackSet::discover(&PackRoots::from_env()))
        .stages(stages)
        .load()?;
    install(content);
    Ok(content)
}

/// Pins a registry on the current thread until dropped (see [`pin`]).
#[must_use = "the pin lasts only while the guard lives"]
pub struct PinGuard {
    previous: Option<&'static ContentRegistry>,
    _not_send: PhantomData<*const ()>,
}

/// Make `content` this thread's [`current`] registry until the guard drops,
/// restoring whatever was pinned before. Worker pools pin the submitting
/// thread's registry around every job; a test pins the registry it built.
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

/// The registry pinned on this thread, if any.
pub fn pinned() -> Option<Content> {
    THREAD.with(Cell::get).pinned.map(Content)
}

/// Switch the content a world runs on: on a thread with a pinned registry
/// (a test, a thread serving its own world) the pin is replaced for the rest
/// of that pin's scope; otherwise `content` is installed for the process.
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

/// The registry for a world that switched `disabled` off, derived from the
/// current registry's installed packs with `stages` as the extra stages. The
/// current registry itself when it already has exactly that enablement;
/// otherwise a previously built registry for the same set is reused before a
/// new one is built. Does NOT make the result current — see [`activate`].
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

/// Registries for tests: the shared default the test fallback installs, and
/// fixture registries a test builds and pins for itself.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::path::Path;
    use std::sync::OnceLock;

    use super::{Content, ContentRegistry};
    use crate::assets::{PackRoots, PackSet};

    /// The registry test binaries fall back to: base assets plus whatever
    /// [`PackRoots::from_env`] discovers, built once per process. Panics with
    /// the full load report when the shipped content does not load.
    pub fn default_content() -> Content {
        static DEFAULT: OnceLock<Content> = OnceLock::new();
        *DEFAULT.get_or_init(|| {
            ContentRegistry::load(PackSet::discover(&PackRoots::from_env()))
                .unwrap_or_else(|e| panic!("{e}"))
        })
    }

    /// A registry over the base assets plus EXACTLY the packs under `mods` —
    /// build it, [`pin`](super::pin) it, run the test body in-process.
    /// Panics with the full load report when the fixture does not load.
    pub fn with_mods(mods: &Path) -> Content {
        ContentRegistry::load(PackSet::discover(&PackRoots::with_mods([
            mods.to_path_buf()
        ])))
        .unwrap_or_else(|e| panic!("fixture content failed to load:\n{e}"))
    }
}
