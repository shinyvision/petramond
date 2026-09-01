//! The engine's OWN hand-swing animation, loaded from the same authored
//! files a mod pack ships — the vanilla punch is a consumer of the shared
//! animation layer ([`mod_api::animation`]), not a parallel system.
//!
//! Three documents under `assets/animations/` (pack-overridable through the
//! ordinary asset roots, like a model or a texture):
//!
//! - `hand_swing.json` — the held-item FIRST-PERSON channel, plus the
//!   vanilla pacing: `window_mine` runs the mining sawtooth, `window_attack`
//!   the one-shot punch/jab.
//! - `hand_arm.json`  — the bare-arm jab, played in the arm's local frame
//!   (per-key `origin` hinges when the file keys one).
//! - `hand_player.json` — the THIRD-PERSON body bone channels, layered over
//!   the walk gait per each bone's compose/replace mode.
//!
//! The files are the truth: the engine samples what they key and nothing
//! else, so re-authoring one changes the vanilla motion — deliberately.
//! Nothing in the engine or its tests pins their shape.
//!
//! What deliberately STAYS procedural: the gaze-driven layers. Head-look,
//! the twist's gaze compensation, and the punch's aim-with-pitch on the
//! swinging shoulder all depend on where THIS viewer's player is looking —
//! per-frame state a keyframe file cannot carry. Clips author motion; gaze
//! layers ride on top (player_model.rs).
//!
//! A missing or refused file degrades like a missing model: that channel
//! plays NOTHING (rest), loudly logged — never a compiled stand-in, which
//! would be the two-systems debt growing back.

use std::sync::LazyLock;

use mod_api::animation::{BodyCurve, PoseCurve};

/// The classic vanilla swing rate (4.2 swings/s) — the fallback pacing when
/// `hand_swing.json` is missing or carries no windows, so a broken asset
/// tree still paces the (then motionless) swing state machine sanely.
const FALLBACK_WINDOW: f32 = 1.0 / 4.2;

pub(crate) struct VanillaSwing {
    /// Held-item first-person channel (`hand_swing.json`).
    pub held: Option<PoseCurve>,
    /// Bare-arm first-person channel (`hand_arm.json`).
    pub arm: Option<PoseCurve>,
    /// Third-person body channel (`hand_player.json`).
    pub body: Option<BodyCurve>,
    /// The body channel's bone names with `left`/`right` swapped, index-
    /// aligned with `body`'s entries — resolved once here so the off-hand
    /// mirror renames nothing per frame.
    pub body_mirrored: Vec<String>,
    /// Seconds per mining-loop swing (`window_mine`).
    pub loop_seconds: f32,
    /// Seconds per one-shot punch/jab (`window_attack`).
    pub one_shot_seconds: f32,
}

/// The rig's chirality rename: `left*` ↔ `right*`, anything else its own
/// mirror (the torso poses both hands' swings).
fn mirrored_bone(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("left") {
        format!("right{rest}")
    } else if let Some(rest) = name.strip_prefix("right") {
        format!("left{rest}")
    } else {
        name.to_string()
    }
}

fn load_pose(rel: &str) -> Option<PoseCurve> {
    let (bytes, path) = petramond_world::assets::read_bytes(rel)?;
    let curve = PoseCurve::from_harness(std::str::from_utf8(&bytes).ok()?);
    if curve.is_none() {
        log::error!("vanilla swing animation {path:?} did not parse — that channel plays nothing");
    }
    curve
}

fn load_body(rel: &str) -> Option<BodyCurve> {
    let (bytes, path) = petramond_world::assets::read_bytes(rel)?;
    let curve = BodyCurve::from_harness(std::str::from_utf8(&bytes).ok()?);
    if curve.is_none() {
        log::error!("vanilla swing animation {path:?} did not parse — that channel plays nothing");
    }
    curve
}

static VANILLA_SWING: LazyLock<VanillaSwing> = LazyLock::new(|| {
    let held = load_pose("animations/hand_swing.json");
    if held.is_none() {
        log::error!("animations/hand_swing.json missing — the vanilla hand swing will not animate");
    }
    let loop_seconds = held
        .as_ref()
        .and_then(|c| c.window_mine())
        .unwrap_or(FALLBACK_WINDOW);
    let one_shot_seconds = held
        .as_ref()
        .and_then(|c| c.window_attack())
        .unwrap_or(FALLBACK_WINDOW);
    let body = load_body("animations/hand_player.json");
    let body_mirrored = body
        .as_ref()
        .map(|c| c.entries().iter().map(|(n, _)| mirrored_bone(n)).collect())
        .unwrap_or_default();
    VanillaSwing {
        held,
        arm: load_pose("animations/hand_arm.json"),
        body,
        body_mirrored,
        loop_seconds,
        one_shot_seconds,
    }
});

/// The loaded vanilla swing, for the process lifetime.
pub(crate) fn vanilla_swing() -> &'static VanillaSwing {
    &VANILLA_SWING
}
