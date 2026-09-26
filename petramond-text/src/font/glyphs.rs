//! The glyph store: which chain member covers each codepoint, every
//! advance up front, and bitmaps rasterized the first time a glyph is drawn.
//!
//! Loading a face only records its coverage and advances — the cheap part —
//! so a font covering tens of thousands of codepoints (a CJK face, emoji)
//! costs a table walk, not tens of thousands of rasterizations. Measurement
//! never rasterizes; the first [`GlyphSet::glyph`] call for a codepoint
//! rasterizes it and places it in the fixed-size atlas, bumping
//! [`GlyphSet::revision`] so hosts know to re-upload.

use super::atlas::Shelves;
use super::raster::{self, Face, RawGlyph};
use super::{hollow_box, place, FontError, Glyph};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Codepoints below this resolve through a dense table (Latin, Greek,
/// Cyrillic — the text the UI actually draws); the rest through a map.
const DENSE_LIMIT: u32 = 0x800;
const NO_GLYPH: u32 = u32::MAX;

/// One member of the fallback chain.
#[derive(Debug)]
pub(super) enum Source {
    Face(Face),
    /// The built-in 5×7 table.
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

/// One covered codepoint.
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
    /// The primary face's baseline, which every glyph is placed against.
    baseline: i32,
    /// What unknown codepoints draw; built by [`GlyphSet::seal_fallback`].
    fallback: OnceLock<Glyph>,
    atlas: Mutex<Shelves>,
    atlas_size: (u32, u32),
    revision: AtomicU64,
}

impl GlyphSet {
    /// A store over `sources` where `covered` names each codepoint's chain
    /// member (earlier members first; a codepoint appears once).
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
        // Room for every covered glyph plus the fallback, at the largest
        // cell any member can produce.
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

    /// Whether `ch` has its own glyph (false = it draws the fallback).
    pub fn has(&self, ch: char) -> bool {
        self.index(ch).is_some()
    }

    /// How many codepoints resolve to a real glyph (fallback excluded).
    pub fn count(&self) -> usize {
        self.slots.len()
    }

    /// `ch`'s pen advance — never rasterizes.
    pub fn advance(&self, ch: char) -> i32 {
        match self.index(ch) {
            Some(i) => self.slots[i].advance,
            None => self.fallback().advance,
        }
    }

    /// The widest advance of any glyph, the fallback included.
    pub fn max_advance(&self) -> i32 {
        let widest = self.slots.iter().map(|s| s.advance).max().unwrap_or(1);
        widest.max(self.fallback().advance).max(1)
    }

    /// `ch`'s glyph, rasterized and placed in the atlas on first use.
    pub fn glyph(&self, ch: char) -> &Glyph {
        match self.index(ch) {
            Some(i) => self.slot_glyph(i),
            None => self.fallback(),
        }
    }

    /// `ch`'s glyph when it has its own (no fallback).
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

    /// Fix the glyph unknown codepoints draw. Called once, while assembling.
    pub fn seal_fallback(&self, glyph: Glyph) {
        let glyph = self.insert(glyph);
        // Only the assembling thread can reach an unsealed set.
        let _ = self.fallback.set(glyph);
    }

    /// Give `glyph` its atlas rect. A glyph the full atlas cannot take keeps
    /// its advance but draws nothing (the atlas is sized for every covered
    /// glyph, so only overhanging ink can run it out).
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

    /// Bumped whenever a glyph lands in the atlas.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Every glyph rasterized so far (the fallback included).
    pub fn rasterized(&self) -> impl Iterator<Item = &Glyph> {
        self.slots
            .iter()
            .filter_map(|s| s.glyph.get())
            .chain(self.fallback.get())
    }
}

/// The glyph unknown codepoints share: U+FFFD from the chain, else a hollow
/// box over the body — never a blank, so a missing glyph shows.
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
