//! Player and creature animation beyond single clips: the expression
//! language animator graphs are written in, local-space poses that layers
//! blend and add, left/right mirroring, and the animator graph runtime.
//!
//! Pure and deterministic: no platform, no clock of its own. A driver feeds
//! inputs, events and `dt`; the runtime answers a pose and the markers its
//! clips crossed.

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
