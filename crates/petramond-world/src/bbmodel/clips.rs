//! The animation clip names the engine looks up in a mob model BY CONVENTION.
//!
//! A model author names a clip; the engine recognises a handful of names and
//! drives them itself, so a species gains a behaviour by authoring the clip,
//! never by touching code. Every recognised name is declared here, once, so
//! the sim (which schedules clips) and the renderer (which samples them) can
//! never disagree on a spelling. Any other clip name is free for AI nodes to
//! start by name (`melee_attack`'s `animation`, a scripted decision's
//! `animation`) and for mods to drive through the named-animation seams.

/// Locomotion gait, played while the body moves; the sim advances its clock
/// by the species' walk rate times the decided speed scale.
pub const WALK: &str = "walk";
/// The player rig's sneaking gait, cross-faded with [`WALK`] by sneak depth.
pub const SNEAK: &str = "sneak";
/// Recoil one-shot the renderer samples from the replicated hurt flash;
/// composes over the current pose, never replaces it.
pub const HURT: &str = "hurt";
/// Looping detail layer (blinks, ear flicks) the sim keeps running on its own
/// per-mob clock across gait changes. Non-looping or zero-length = ignored.
pub const AMBIENT: &str = "ambient";
/// Prefix of the one-shot idle clips the `idle_anim` brain node picks from by
/// name-sorted index.
pub const IDLE_PREFIX: &str = "idle_";

/// Whether `name` is one of the `idle_*` clips.
pub fn is_idle(name: &str) -> bool {
    name.starts_with(IDLE_PREFIX)
}
