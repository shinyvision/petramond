//! The VIEW CLAIMS a client instance publishes — where the view looks from,
//! what chrome it draws, which perspective presents, and its presentation
//! overrides of the replicated shader params — plus the fold that resolves
//! them across mods.
//!
//! A claim is RETAINED: a mod writes one and it stands until that mod takes it
//! back, exactly like a held pose. That is why release is its own call rather
//! than a sentinel value — the frame a mod lets go of the camera has to present
//! immediately.
//!
//! The fold runs over the runtime's mod-id order (`ClientModRuntime`'s
//! `claim_stores`), the same sequence the body claims resolve in, so two packs
//! with an opinion about the same thing reach one answer whatever order they
//! loaded in.

use std::collections::BTreeMap;

/// Where a mod wants the view to look from: a world point, a look in radians,
/// and optionally a vertical field of view (`None` keeps the player's own).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewCameraClaim {
    /// A world point, or with `anchor` an offset from its presented feet.
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: Option<f32>,
    pub anchor: Option<mod_api::EntityRef>,
}

/// One opinion about the on-screen chrome. `None` per field = no opinion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewChromeClaim {
    pub hud: Option<bool>,
    pub hands: Option<bool>,
    pub crosshair: Option<bool>,
}

/// Everything ONE client mod claims about the view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewClaims {
    pub camera: Option<ViewCameraClaim>,
    pub chrome: ViewChromeClaim,
    /// `None` = no claim; the player's own perspective toggle stands.
    pub third_person: Option<bool>,
    /// This mod's presentation overrides of named shader params. A key dropped
    /// from the set falls back to the replicated value.
    pub env: BTreeMap<String, [f32; 4]>,
    /// The size the world frame renders at; `None` = no claim.
    pub frame_size: Option<[u32; 2]>,
    /// Whose view presents; `None` = no claim (the local body's).
    pub subject: Option<mod_api::PlayerId>,
}

/// Every client mod's view claims, resolved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewFold {
    pub camera: Option<ViewCameraClaim>,
    pub chrome: ViewChromeClaim,
    pub third_person: Option<bool>,
    pub env: BTreeMap<String, [f32; 4]>,
    pub frame_size: Option<[u32; 2]>,
    pub subject: Option<mod_api::PlayerId>,
}

/// HIDING WINS: a single `Some(false)` settles the field, `Some(true)` only
/// beats "no opinion".
///
/// A pack asking for a clean frame is asking for something it cannot get any
/// other way, while a pack asking for the HUD is asking for the default — so
/// the asymmetry is the only resolution that lets both run at once.
fn hide_wins(held: Option<bool>, claim: Option<bool>) -> Option<bool> {
    match (held, claim) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, None) => None,
    }
}

/// Resolve `claims`, which arrive in the runtime's mod-id fold order.
///
/// Chrome folds by [`hide_wins`]. The camera, the perspective and each env key
/// go to the LAST claimant in the order — the rule the body claims settle a
/// contested hand with, so one sequence explains every contested claim.
pub fn fold<'a>(claims: impl Iterator<Item = &'a ViewClaims>) -> ViewFold {
    let mut out = ViewFold::default();
    for mine in claims {
        if mine.camera.is_some() {
            out.camera = mine.camera;
        }
        out.chrome.hud = hide_wins(out.chrome.hud, mine.chrome.hud);
        out.chrome.hands = hide_wins(out.chrome.hands, mine.chrome.hands);
        out.chrome.crosshair = hide_wins(out.chrome.crosshair, mine.chrome.crosshair);
        if mine.third_person.is_some() {
            out.third_person = mine.third_person;
        }
        if mine.frame_size.is_some() {
            out.frame_size = mine.frame_size;
        }
        if mine.subject.is_some() {
            out.subject = mine.subject;
        }
        for (key, value) in &mine.env {
            out.env.insert(key.clone(), *value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(x: f64) -> Option<ViewCameraClaim> {
        Some(ViewCameraClaim {
            pos: [x, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov_y: None,
            anchor: None,
        })
    }

    #[test]
    fn hiding_beats_showing_whatever_the_order() {
        let hide = ViewClaims {
            chrome: ViewChromeClaim {
                hud: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let show = ViewClaims {
            chrome: ViewChromeClaim {
                hud: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(fold([&hide, &show].into_iter()).chrome.hud, Some(false));
        assert_eq!(fold([&show, &hide].into_iter()).chrome.hud, Some(false));
        // No opinion anywhere leaves the engine's own answer standing.
        assert_eq!(fold([&ViewClaims::default()].into_iter()).chrome.hud, None);
        assert_eq!(fold([&show].into_iter()).chrome.hud, Some(true));
    }

    #[test]
    fn the_last_claimant_in_the_order_owns_the_camera_and_each_env_key() {
        let mut first = ViewClaims {
            camera: camera(1.0),
            third_person: Some(true),
            ..Default::default()
        };
        first.env.insert("a".into(), [1.0; 4]);
        first.env.insert("b".into(), [1.0; 4]);
        let mut second = ViewClaims {
            camera: camera(2.0),
            ..Default::default()
        };
        second.env.insert("b".into(), [2.0; 4]);

        let folded = fold([&first, &second].into_iter());
        assert_eq!(folded.camera, camera(2.0));
        // A mod with no opinion never takes a claim away from one that has it.
        assert_eq!(folded.third_person, Some(true));
        assert_eq!(folded.env.get("a"), Some(&[1.0; 4]));
        assert_eq!(folded.env.get("b"), Some(&[2.0; 4]));
    }
}
