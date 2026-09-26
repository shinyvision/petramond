//! Mod sounds and particle bursts: one-shot, spatial, mob-pinned, retuned,
//! stopped.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::legality::prelude::*;

host_domain! {
    /// Mod sounds and particle bursts: one-shot, spatial, mob-pinned, retuned,
    /// stopped.
    SoundCall {
        /// Play a sound by `sounds.json` key (namespaced for pack sounds), routed
        /// through the tick→presentation channel — the sim never touches audio.
        /// `pos` attenuates by the sound row's `attenuation_distance`; `None`
        /// plays at full volume. `false` = unknown key. → [`HostRet::Bool`](crate::HostRet::Bool).
        EmitSound {
            key: String,
            pos: Option<[f64; 3]>,
        } => legal(SERVER, Sim, Write),
        /// Start a positional sound at a fixed world position. The host resolves
        /// `key` through `sounds.json`, queues a deterministic presentation
        /// command, and returns a session sound handle. `0` means the key was
        /// unknown or the parameters were invalid, so no sound was queued.
        /// `volume` is a linear multiplier, `pitch` is playback speed, and travel
        /// distance comes from the sound row's `attenuation_distance`.
        /// → [`HostRet::U64`](crate::HostRet::U64).
        SoundPlayAt {
            key: String,
            pos: [f64; 3],
            volume: f32,
            pitch: f32,
        } => legal(SERVER, Sim, Write),
        /// Start a positional sound pinned to a live mob's stable [`MobSnapshot::id`](crate::MobSnapshot::id).
        /// The app/audio side follows that mob's per-frame presentation position; if
        /// the mob despawns, the sound finishes at its last known position. Returns
        /// `0` when the sound key or mob id is unknown, or parameters are invalid.
        /// Travel distance comes from the sound row's `attenuation_distance`.
        /// → [`HostRet::U64`](crate::HostRet::U64).
        SoundPlayOnMob {
            mob_id: u64,
            key: String,
            volume: f32,
            pitch: f32,
        } => legal(SERVER, Sim, Write),
        /// Stop a spatial sound previously started by this session handle. Unknown
        /// handles are a no-op. → [`HostRet::Unit`](crate::HostRet::Unit).
        SoundStop {
            handle: u64,
        } => legal(SERVER, Sim, Write),
        /// Fire a ONE-SHOT particle burst: `key` names a `particle_emitters.json`
        /// BURST bundle (e.g. the core `petramond:water_splash`), spawned at `pos`
        /// for every client. `intensity` scales the particle count through the
        /// bundle's `count_per_intensity` (the core water splash passes blocks
        /// fallen). Fire-and-forget presentation, like `EmitSound`. →
        /// [`HostRet::Bool`](crate::HostRet::Bool) (`false` = unknown key or not a burst bundle).
        EmitterBurst {
            key: String,
            pos: [f64; 3],
            intensity: f32,
            /// Which way the event pushes, for a bundle with an `along_speed` (a
            /// struck face's normal).
            direction: Option<[f32; 3]>,
            /// What the particles are cut from; `None` = the bundle's own look.
            texture: Option<crate::ParticleTexture>,
        } => legal(SERVER, Sim, Write),
        /// Retune a live spatial sound started by [`SoundPlayAt`] or
        /// [`SoundPlayOnMob`]: `volume` (linear multiplier) and `pitch`
        /// (playback speed) replace the values the play was started with; the
        /// sound keeps its source and its place in the clip. The seam for a
        /// sound whose loudness FOLLOWS a quantity the mod integrates — a cart
        /// rolling faster, a furnace roaring up — on a row that `loop`s, so one
        /// play carries the whole ride and nothing restarts. Unknown or finished
        /// handles are a no-op; non-finite or negative values are refused.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ///
        /// [`SoundPlayAt`]: Self::SoundPlayAt
        /// [`SoundPlayOnMob`]: Self::SoundPlayOnMob
        SoundSet {
            handle: u64,
            volume: f32,
            pitch: f32,
        } => legal(SERVER, Sim, Write),
    }
}
