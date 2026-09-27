pub mod expr;
pub mod graph;
mod inertia;
pub mod library;
pub mod pose;
pub mod runtime;
#[cfg(any(test, feature = "test-support"))]
pub mod test_rig;

pub use graph::{Ease, EventId, Graph, ParamId, Pick, SlotId};
pub use library::{ClipId, ClipLibrary};
pub use pose::{LocalPose, MirrorMap};
pub use runtime::{Animator, FiredMarker, PlayId, PlaySpec, PlayState, Playing};
