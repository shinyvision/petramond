//! The per-worker buffers a section build reuses: the greedy-merge scratch,
//! the box-set emitter's buffers, the neighbourhood's occupancy and seal
//! scratch, and the vertex streams themselves.
//!
//! Each mesh worker thread keeps one [`MeshScratch`]. A build takes it out
//! through a [`ScratchLease`] and the lease puts it back when dropped — on
//! success, on cancellation, and while unwinding from a panic alike — so no
//! exit path throws the ~400 KB greedy scratch or the grown streams away and
//! makes the next build on that thread re-allocate and re-zero them.
//!
//! The streams are built in place here and the finished mesh receives
//! exact-size copies: one allocation per non-empty stream instead of a chain
//! of doubling reallocations, and the grown capacity stays on the worker
//! rather than following the mesh to the renderer.

use std::cell::{Cell, RefCell};
use std::ops::{Deref, DerefMut};

use petramond_world::block::ShapeBox;

use super::super::boxset::BoxSetScratch;
use super::super::greedy::GreedyScratch;
use super::super::vertex::{ContactShadowVertex, ModelVertex, Vertex};

/// Every vertex stream one section build fills.
#[derive(Default)]
pub(super) struct Streams {
    pub(super) opaque: Vec<Vertex>,
    /// Leaf faces that sit against another cell of the SAME leaves. They are
    /// the ONLY thing the far (simplified-canopy) LOD drops, so they are
    /// emitted into their own buffer and appended to `opaque` last — which
    /// makes the far LOD exactly the opaque stream's leading prefix, built in
    /// this one traversal instead of a second whole-section pass.
    pub(super) leaf_interior: Vec<Vertex>,
    pub(super) transparent: Vec<Vertex>,
    pub(super) transparent_two_sided: Vec<Vertex>,
    pub(super) translucent: Vec<Vertex>,
    pub(super) model: Vec<ModelVertex>,
    pub(super) model_idx: Vec<u32>,
    pub(super) model_blend_idx: Vec<u32>,
    pub(super) contact: Vec<ContactShadowVertex>,
}

impl Streams {
    /// Empty every stream, keeping its capacity.
    fn clear(&mut self) {
        self.opaque.clear();
        self.leaf_interior.clear();
        self.transparent.clear();
        self.transparent_two_sided.clear();
        self.translucent.clear();
        self.model.clear();
        self.model_idx.clear();
        self.model_blend_idx.clear();
        self.contact.clear();
    }
}

/// The box-set emitter's reusable buffers.
#[derive(Default)]
pub(super) struct BoxBuffers {
    pub(super) scratch: BoxSetScratch,
    /// The cell's own resolved boxes.
    pub(super) cell: Vec<ShapeBox>,
    /// The snow blanket a `snow_bedded` cell is drawn standing in.
    pub(super) bed: Vec<ShapeBox>,
}

/// The neighbourhood queries' scratch, borrowed shared by the
/// [`Neighbourhood`](super::neighbourhood::Neighbourhood) (its queries take
/// `&self`).
#[derive(Default)]
pub(super) struct NeighbourScratch {
    /// A neighbour's resolved occupancy boxes.
    pub(super) occupancy: RefCell<Vec<ShapeBox>>,
    /// A cell's boxes and the subtraction scratch for the floor-seal test.
    pub(super) seal: RefCell<(Vec<ShapeBox>, BoxSetScratch)>,
}

/// Everything a section build reuses. See the module doc.
#[derive(Default)]
pub(super) struct MeshScratch {
    pub(super) greedy: GreedyScratch,
    pub(super) boxes: BoxBuffers,
    pub(super) neighbour: NeighbourScratch,
    pub(super) out: Streams,
}

thread_local! {
    static SCRATCH: Cell<Option<Box<MeshScratch>>> = const { Cell::new(None) };
}

/// This thread's [`MeshScratch`], out of its slot for the length of one
/// build and put back on drop.
pub(super) struct ScratchLease(Option<Box<MeshScratch>>);

impl ScratchLease {
    /// Take this thread's scratch (a fresh one on first use, or if a build on
    /// this thread already holds it), with the streams emptied for a new build.
    pub(super) fn take() -> Self {
        let mut scratch = SCRATCH.with(Cell::take).unwrap_or_default();
        scratch.out.clear();
        Self(Some(scratch))
    }
}

impl Deref for ScratchLease {
    type Target = MeshScratch;

    fn deref(&self) -> &MeshScratch {
        self.0
            .as_ref()
            .expect("a lease holds its scratch until dropped")
    }
}

impl DerefMut for ScratchLease {
    fn deref_mut(&mut self) -> &mut MeshScratch {
        self.0
            .as_mut()
            .expect("a lease holds its scratch until dropped")
    }
}

impl Drop for ScratchLease {
    fn drop(&mut self) {
        if let Some(scratch) = self.0.take() {
            // `try_with`: a lease dropped during thread teardown has nowhere
            // to return to, and the scratch is simply freed.
            let _ = SCRATCH.try_with(|slot| slot.set(Some(scratch)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_returns_its_buffers_on_every_exit_path() {
        let grown = {
            let mut lease = ScratchLease::take();
            lease.out.opaque.reserve(1024);
            lease.out.opaque.push(bytemuck::Zeroable::zeroed());
            lease.greedy.begin();
            lease.out.opaque.capacity()
        };
        // The next build on this thread gets the same buffers back, emptied.
        let lease = ScratchLease::take();
        assert!(lease.out.opaque.is_empty());
        assert_eq!(lease.out.opaque.capacity(), grown);
        assert!(!lease.greedy.faces.is_empty(), "greedy scratch kept");
        drop(lease);

        // A panicking build still puts the scratch back while unwinding.
        let caught = std::panic::catch_unwind(|| {
            let mut lease = ScratchLease::take();
            lease.out.transparent.reserve(512);
            panic!("build failed");
        });
        assert!(caught.is_err());
        let lease = ScratchLease::take();
        assert!(lease.out.transparent.capacity() >= 512);
        assert!(!lease.greedy.faces.is_empty());
    }
}
