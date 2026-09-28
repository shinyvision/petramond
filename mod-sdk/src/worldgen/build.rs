//! Generated structures that span many sections.
//!
//! A structure is derived ONCE per site into a [`Plan`] from positional reads only (terrain
//! heights, surface biomes, the site's own rng), published to the shared memo, and clipped into
//! every section it touches ([`IndexedPlan::emit`]). Sections generate in any order on any
//! worker, so nothing a plan decides may depend on which sections happen to exist; per-section
//! snapshots are for clipping only.
//!
//! Blocks are written through [`Material`]s in the vocabulary of a structure template palette
//! (`facing`, `half`, `axis`, `open`), so shape families lay out stairs, slabs, logs and doors
//! exactly as a template would. Material families (a wood's planks, slab, stairs, fence and log)
//! come from `petramond:material` block row data ([`Families`]), so a pack that adds a wood or
//! stone makes it available to every structure without code.

mod families;
mod frame;
mod heights;
mod material;
mod noise;
mod plan;
mod raster;
mod rng;
mod roof;
mod site;
mod span;

pub use families::{Families, MATERIAL_DATA_KEY};
pub use frame::Frame;
pub use heights::{surface_biomes, Heights};
pub use material::{Axis, Dir, Form, Half, Material, Name};
pub use noise::{Noise2, Noise3};
pub use plan::{IndexedPlan, Plan};
pub use raster::{closed_ring, depths_inside, segment, RingField, SegmentCell};
pub use rng::Draw;
pub use roof::{a_frame, gable, lean_to, RoofStyle};
pub use site::{Deferred, Derived, PlanCache, Site, SiteGrid};
pub use span::{span, Span, SpanCell};

#[cfg(test)]
mod tests;
