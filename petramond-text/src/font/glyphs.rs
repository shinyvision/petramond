use super::atlas::Shelves;
use super::raster::{self, Face, RawGlyph};
use super::{hollow_box, place, FontError, Glyph};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const DENSE_LIMIT: u32 = 0x800;
const NO_GLYPH: u32 = u32::MAX;

#[derive(Debug)]
pub(super) enum Source {
    Face(Face),
    Builtin,
}

impl Source {
    fn advance(&self, ch: char) -> i32 {
        match self {
            Source::Face(face) => face.advance(ch),
            Source::Builtin => crate::builtin::ADVANCE,
        }
    }

    fn raster(&self, ch: char) -> RawGlyph {
        match self {
            Source::Face(face) => face.raster(ch),
            Source::Builtin => raster::builtin_raw(ch),
        }
    }

    fn line_h(&self) -> i32 {
        match self {
            Source::Face(face) => face.metrics.line_h,
            Source::Builtin => raster::builtin_metrics().line_h,
        }
    }
}

#[derive(Debug)]
struct Slot {
    ch: char,
    source: u16,
    advance: i32,
    glyph: OnceLock<Glyph>,
}

#[derive(Debug)]
pub(super) struct GlyphSet {
    sources: Vec<Source>,
    slots: Vec<Slot>,
    dense: Vec<u32>,
    sparse: HashMap<char, u32>,
    baseline: i32,
    fallback: OnceLock<Glyph>,
    atlas: Mutex<Shelves>,
    atlas_size: (u32, u32),
    revision: AtomicU64,
}

impl GlyphSet {
    pub fn new(
        sources: Vec<Source>,
        covered: Vec<(char, u16)>,
        baseline: i32,
    ) -> Result<GlyphSet, FontError> {
        let slots: Vec<Slot> = covered
            .into_iter()
            .map(|(ch, source)| Slot {
                ch,
                source,
                advance: sources[source as usize].advance(ch),
                glyph: OnceLock::new(),
            })
            .collect();
        let mut dense = vec![NO_GLYPH; DENSE_LIMIT as usize];
        let mut sparse = HashMap::new();
        for (i, slot) in slots.iter().enumerate() {
            match dense.get_mut(slot.ch as usize) {
                Some(entry) => *entry = i as u32,
                None => {
                    sparse.insert(slot.ch, i as u32);
                }
            }
        }
        let cell_w = slots.iter().map(|s| s.advance).max().unwrap_or(1).max(1);
        let cell_h = sources.iter().map(Source::line_h).max().unwrap_or(1).max(1);
        let atlas = Shelves::for_glyphs(slots.len() + 1, (cell_w as u32 + 1, cell_h as u32 + 1))?;
        let atlas_size = atlas.size();
        Ok(GlyphSet {
            sources,
            slots,
            dense,
            sparse,
            baseline,
            fallback: OnceLock::new(),
            atlas: Mutex::new(atlas),
            atlas_size,
            revision: AtomicU64::new(0),
        })
    }

    fn index(&self, ch: char) -> Option<usize> {
        let i = match self.dense.get(ch as usize) {
            Some(&i) => i,
            None => *self.sparse.get(&ch)?,
        };
        (i != NO_GLYPH).then_some(i as usize)
    }

    pub fn has(&self, ch: char) -> bool {
        self.index(ch).is_some()
    }

    pub fn count(&self) -> usize {
        self.slots.len()
    }

    pub fn advance(&self, ch: char) -> i32 {
        match self.index(ch) {
            Some(i) => self.slots[i].advance,
            None => self.fallback().advance,
        }
    }

    pub fn max_advance(&self) -> i32 {
        let widest = self.slots.iter().map(|s| s.advance).max().unwrap_or(1);
        widest.max(self.fallback().advance).max(1)
    }

    pub fn glyph(&self, ch: char) -> &Glyph {
        match self.index(ch) {
            Some(i) => self.slot_glyph(i),
            None => self.fallback(),
        }
    }

    pub fn own_glyph(&self, ch: char) -> Option<&Glyph> {
        self.index(ch).map(|i| self.slot_glyph(i))
    }

    fn slot_glyph(&self, i: usize) -> &Glyph {
        let slot = &self.slots[i];
        slot.glyph.get_or_init(|| {
            let raw = self.sources[slot.source as usize].raster(slot.ch);
            let mut glyph = place(&raw, self.baseline);
            glyph.advance = slot.advance;
            self.insert(glyph)
        })
    }

    fn fallback(&self) -> &Glyph {
        self.fallback
            .get()
            .expect("the fallback is sealed when the font assembles")
    }

    pub fn seal_fallback(&self, glyph: Glyph) {
        let glyph = self.insert(glyph);
        let _ = self.fallback.set(glyph);
    }

    fn insert(&self, mut glyph: Glyph) -> Glyph {
        let [_, _, w, h] = glyph.bounds;
        if w <= 0 || h <= 0 {
            return glyph;
        }
        let placed = self
            .atlas
            .lock()
            .map(|mut atlas| atlas.place(w as u32, h as u32))
            .unwrap_or(None);
        match placed {
            Some([x, y]) => {
                glyph.atlas = [x, y, w as u32, h as u32];
                self.revision.fetch_add(1, Ordering::Release);
            }
            None => {
                glyph.bitmap.clear();
                glyph.bounds = [0, 0, 0, 0];
            }
        }
        glyph
    }

    pub fn atlas_size(&self) -> (u32, u32) {
        self.atlas_size
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn rasterized(&self) -> impl Iterator<Item = &Glyph> {
        self.slots
            .iter()
            .filter_map(|s| s.glyph.get())
            .chain(self.fallback.get())
    }
}

pub(super) fn fallback_glyph(
    set: &GlyphSet,
    boxed_fallback: bool,
    body: (i32, i32),
    line_h: i32,
) -> Glyph {
    match set.own_glyph('\u{FFFD}') {
        Some(g) if !boxed_fallback || g.bounds[3] > 0 => g.clone(),
        _ => {
            let advance = set.own_glyph('?').map_or(line_h / 2, |g| g.advance).max(3);
            hollow_box(advance, body)
        }
    }
}
