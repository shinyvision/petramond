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

#[derive(Default)]
pub(super) struct Streams {
    pub(super) opaque: Vec<Vertex>,
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

#[derive(Default)]
pub(super) struct BoxBuffers {
    pub(super) scratch: BoxSetScratch,
    pub(super) cell: Vec<ShapeBox>,
    pub(super) bed: Vec<ShapeBox>,
}

#[derive(Default)]
pub(super) struct NeighbourScratch {
    pub(super) occupancy: RefCell<Vec<ShapeBox>>,
    pub(super) seal: RefCell<(Vec<ShapeBox>, BoxSetScratch)>,
}

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

pub(super) struct ScratchLease(Option<Box<MeshScratch>>);

impl ScratchLease {
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
        let lease = ScratchLease::take();
        assert!(lease.out.opaque.is_empty());
        assert_eq!(lease.out.opaque.capacity(), grown);
        assert!(!lease.greedy.faces.is_empty(), "greedy scratch kept");
        drop(lease);

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
