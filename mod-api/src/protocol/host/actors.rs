//! Mob actors acting on the world: digging, placing, interacting, aiming.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::{BlockRecord, EntityRef};
use crate::legality::prelude::*;

host_domain! {
    /// Mob actors acting on the world: digging, placing, interacting, aiming.
    ActorCall {
        /// One tick of `actor` (a live mob) digging the block at `pos` with the
        /// tool in slot `tool_slot` of its own carried container (`None` = its
        /// bare hands), under the mining rules a player's break follows: the
        /// block's hardness, the tool's kind, tier and speed, and the harvest gate
        /// for drops. The dig accrues only while the actor keeps calling on
        /// consecutive ticks for the same block and tool; a gap, another target or
        /// another tool starts over. Every call checks reach from the actor's eye
        /// (its row's `reach`) and a clear line to the block. When the duration
        /// completes the break is queued: `block_break_pre` sees the actor, and
        /// with `collect` the drops (and a broken container's contents) go into
        /// the actor's container, the rest scattering as usual. Presentation
        /// follows the actor: the crack shows on the block while it digs.
        /// Server only. → [`HostRet::Dig`](crate::HostRet::Dig).
        ActorDig {
            actor: EntityRef,
            pos: [i32; 3],
            tool_slot: Option<u32>,
            collect: bool,
        } => legal(SERVER, Sim, Write),
        /// `actor` (a live mob) builds `record` at `pos` — the whole object the
        /// record anchors — under survival placement rules: reach and a clear
        /// line from its eye, a block beside the object to place it against, the
        /// support its row demands, no body in the way, and `block_place_pre`
        /// seeing the actor. With `pay`, the record's cost (only the missing
        /// parts of a partly built cell) must be carried in the actor's container
        /// and is consumed, and the paid items' data lands in the cell. Without
        /// `pay` the record's block must be the calling mod's own. Valid requests
        /// queue for this tick and report through
        /// [`EventKind::ActorActed`](crate::EventKind::ActorActed). Server only.
        /// → [`HostRet::Place`](crate::HostRet::Place).
        ActorPlace {
            actor: EntityRef,
            pos: [i32; 3],
            record: BlockRecord,
            pay: bool,
        } => legal(SERVER, Sim, Write),
        /// What [`ActorCall::ActorPlace`] would answer if `actor` stood with its
        /// feet at `from`, without placing, paying or queueing anything — the
        /// same rules, so a planner hands its worker only placements the world
        /// accepts. `Queued` = it would be accepted. Refusals that do not depend
        /// on where the actor stands (no face, no support, obstructed, a body in
        /// the way) hold for any stance. Server only. → [`HostRet::Place`](crate::HostRet::Place).
        ActorPlaceCheck {
            actor: EntityRef,
            from: [f64; 3],
            pos: [i32; 3],
            record: BlockRecord,
            pay: bool,
        } => legal(SERVER, Sim, Read),
        /// A live mob uses the block at `pos` the way a player's right-click
        /// does, for what a body does without a screen (a door swings open or
        /// shut, heard and seen by every viewer). The actor must be looking at
        /// the block within its reach, as [`ActorAims`](Self::ActorAims) judges.
        /// Applied at the tick's action point and reported as `actor_acted` with
        /// [`ActorAction::Use`](crate::ActorAction::Use): a block with nothing a
        /// body uses is refused `NothingToDo` there. → [`HostRet::Bool`](crate::HostRet::Bool):
        /// `false` = no such mob, or it cannot click the block from where it
        /// stands. Server only.
        ActorInteract {
            actor: EntityRef,
            pos: [i32; 3],
        } => legal(SERVER, Sim, Write),
        /// Where a live mob, with its feet at each of `from` (at most
        /// `SIM_BATCH_MAX`), would look to work the cell at `pos`: with a
        /// `record`, the point on a face a click places it against —
        /// one that is seen, within reach, and leaves the record's orientation
        /// when clicked looking that way; without, a seen point of the block
        /// standing there (a dig, a use). An actor's actions land only where it
        /// is looking, so this is both the stance test and the gaze to take up.
        /// → [`HostRet::Aims`](crate::HostRet::Aims), parallel to `from`. Server only.
        ActorAims {
            actor: EntityRef,
            from: Vec<[f64; 3]>,
            pos: [i32; 3],
            record: Option<crate::BlockRecord>,
        } => legal(SERVER, Sim, Read),
    }
}
