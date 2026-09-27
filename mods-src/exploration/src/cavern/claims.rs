use mod_sdk::{BlockId, GenCtx, GenWrite};

use super::{is_open, CLAIM_ROWS};

/// Cells this dispatch has spoken for.
///
/// STRUCTURE OUTRANKS DRESSING. A giant's stem base stands on the highest open
/// cell resting on rock, which is exactly what a floor flower asks for, so
/// without a record of what is already taken both are emitted for that cell and
/// whichever write the engine applies last wins — a cave flower growing inside
/// a mushroom stem. The giants are emitted first and everything after them
/// consults this.
///
/// It reaches `CEILING_MARGIN` rows ABOVE the section, which are not ours to
/// write. A curtain hanging in from up there walks through those rows, and a
/// mushroom standing in one stops the run — in the section that owns the root
/// as well as here, because a giant is a pure function of its anchor and both
/// sections reconstruct it. Without that the two disagree about where the run
/// ends and the curtain comes out with nothing solid over its top cell, which
/// is exactly the support the vine rows are declared with.
struct Claims([u64; (256 * CLAIM_ROWS as usize).div_ceil(64)]);

impl Default for Claims {
    fn default() -> Claims {
        Claims([0; (256 * CLAIM_ROWS as usize).div_ceil(64)])
    }
}

impl Claims {
    fn slot(origin: [i32; 3], p: [i32; 3]) -> Option<usize> {
        let (lx, ly, lz) = (p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]);
        let inside =
            (0..16).contains(&lx) && (0..CLAIM_ROWS).contains(&ly) && (0..16).contains(&lz);
        inside.then(|| (ly * 256 + lz * 16 + lx) as usize)
    }

    fn holds(&self, i: usize) -> bool {
        self.0[i / 64] & (1u64 << (i % 64)) != 0
    }

    fn take(&mut self, i: usize) -> bool {
        let free = !self.holds(i);
        self.0[i / 64] |= 1u64 << (i % 64);
        free
    }
}

pub(super) struct Emitter<'a> {
    ctx: &'a GenCtx,
    claims: Claims,
    writes: Vec<GenWrite>,
}

impl<'a> Emitter<'a> {
    pub(super) fn new(ctx: &'a GenCtx) -> Emitter<'a> {
        Emitter {
            ctx,
            claims: Claims::default(),
            writes: Vec::new(),
        }
    }

    pub(super) fn ctx(&self) -> &'a GenCtx {
        self.ctx
    }

    pub(super) fn into_writes(self) -> Vec<GenWrite> {
        self.writes
    }

    pub(super) fn taken(&self, p: [i32; 3]) -> bool {
        Claims::slot(self.ctx.origin_world(), p).is_some_and(|i| self.claims.holds(i))
    }

    pub(super) fn push_if_clear(&mut self, p: [i32; 3], block: BlockId) {
        let Some(i) = Claims::slot(self.ctx.origin_world(), p) else {
            return;
        };
        let ours = self.ctx.block(p).is_some();
        if ours && !is_open(self.ctx, p) {
            return;
        }
        if self.claims.take(i) && ours {
            self.writes.push((p, block));
        }
    }

    pub(super) fn push_over_terrain(&mut self, p: [i32; 3], block: BlockId) {
        let Some(i) = Claims::slot(self.ctx.origin_world(), p) else {
            return;
        };
        if self.claims.take(i) && self.ctx.block(p).is_some() {
            self.writes.push((p, block));
        }
    }

    /// Speak for a cell without writing it. A cascade uses this twice: for
    /// the cavern cells over its own pools, so no flower or sporeshroom is
    /// dressed standing on water (their support is the cell below, which is
    /// about to become a fluid), and for the rock of its own wall, which must
    /// stay exactly as the carver left it.
    pub(super) fn reserve(&mut self, p: [i32; 3]) {
        if let Some(i) = Claims::slot(self.ctx.origin_world(), p) {
            self.claims.take(i);
        }
    }

    #[cfg(test)]
    pub(super) fn writes(&self) -> &[GenWrite] {
        &self.writes
    }
}
