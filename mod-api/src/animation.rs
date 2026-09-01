//! The shared animation-curve layer: the two harness formats and the ONE
//! sampling law, compiled into the engine and into every mod.
//!
//! Two documents exist, both authored by the harness tools and both read by
//! [`crate::json`] on either side of the ABI:
//!
//! - `petramond-swing-animation` (v1) → [`PoseCurve`]: ONE pose channel —
//!   rotation (display-euler XYZ degrees), translation (1/16-block px) and
//!   an optional per-key rotation pivot (`origin`, px) — keyed on phase
//!   0..1. Carries the step's authored pacing: `window_attack` and
//!   `window_mine`, seconds, and optionally WHICH key is the swing's
//!   `impact` — the instant the motion lands. What consumes the channel
//!   decides what it poses: a pack's held-item swing, the engine's own
//!   first-person hand — and what, if anything, happens at the impact.
//! - `petramond-player-animation` (v1) → [`BodyCurve`]: named rig bones,
//!   each Compose (rides the gait) or Replace (a stance), keyed on the same
//!   phase.
//!
//! The sampling law is the smoothstep-between-bracketing-keys rule
//! ([`bracket`]); every consumer interpolates identically, so a curve
//! previewed in a harness, played by a pack, and played by the engine is one
//! motion. The ENGINE'S OWN vanilla hand swing ships as these files
//! (`assets/animations/`), which keeps this layer honest: any drift between
//! "what the engine plays" and "what a mod can author" breaks the engine's
//! own hands first.
//!
//! Parsing is whole-or-nothing: a file this reader does not fully understand
//! answers `None`, and the consumer's compiled fallback stands in — never a
//! half-authoritative curve.

use crate::json::Value;
use crate::{BonePoseData, BonePoseMode};

/// The bracketing keys around `phase`, smoothstep-eased: the pair to blend
/// and how far between them. Keys that share a value (an authored HOLD) read
/// as a freeze because there is nothing to interpolate toward. Generic over
/// the key type so every curve — and a consumer's own compiled tables —
/// share one law. `keys` must be non-empty and sorted by time (both parsers
/// enforce it; a compiled table owes it to itself).
pub fn bracket<K>(keys: &[K], t_of: impl Fn(&K) -> f32, phase: f32) -> (&K, &K, f32) {
    let (mut lo, mut hi) = (&keys[0], &keys[keys.len() - 1]);
    for pair in keys.windows(2) {
        if phase >= t_of(&pair[0]) && phase <= t_of(&pair[1]) {
            lo = &pair[0];
            hi = &pair[1];
            break;
        }
    }
    let u = ((phase - t_of(lo)) / (t_of(hi) - t_of(lo)).max(1e-5)).clamp(0.0, 1.0);
    (lo, hi, u * u * (3.0 - 2.0 * u))
}

/// Componentwise linear mix — the channel blend under [`bracket`]'s eased `u`.
pub fn mix3(a: [f32; 3], b: [f32; 3], u: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * u,
        a[1] + (b[1] - a[1]) * u,
        a[2] + (b[2] - a[2]) * u,
    ]
}

fn numbers3(v: &Value) -> Option<[f32; 3]> {
    let n = v.as_array()?;
    if n.len() != 3 {
        return None;
    }
    let out = [
        n[0].as_f64()? as f32,
        n[1].as_f64()? as f32,
        n[2].as_f64()? as f32,
    ];
    out.iter().all(|c| c.is_finite()).then_some(out)
}

/// A window field: absent is fine (`None` — the consumer's default paces),
/// present-but-broken refuses the whole file.
fn window(doc: &Value, field: &str) -> Option<Option<f32>> {
    match doc.get(field) {
        None => Some(None),
        Some(w) => {
            let w = w.as_f64()? as f32;
            (w.is_finite() && w > 0.0).then_some(Some(w))
        }
    }
}

// ---- the pose channel ------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq)]
struct PoseKey {
    t: f32,
    rotation: [f32; 3],
    translation: [f32; 3],
    origin: [f32; 3],
}

/// One sampled instant of a [`PoseCurve`]: rotation in display-euler XYZ
/// degrees, translation in 1/16-block px, and the rotation pivot (px) the
/// key was authored about. Consumers that cannot carry a pivot (the held-
/// pose ABI has one authored pivot already) require it zero or drop it;
/// the engine's own bare-arm jab hinges on it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PoseSample {
    pub rotation: [f32; 3],
    pub translation: [f32; 3],
    pub origin: [f32; 3],
}

/// One `petramond-swing-animation` document: a single pose channel plus its
/// authored pacing and, when the author marked one, its impact key.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseCurve {
    /// Keys sorted by `t`, first at 0, running to ~1.
    keys: Vec<PoseKey>,
    window_attack: Option<f32>,
    window_mine: Option<f32>,
    /// The phase of the key flagged `"impact": true`, if any.
    impact: Option<f32>,
}

impl PoseCurve {
    /// Parse one harness document, whole. A file carrying the RETIRED
    /// `window_seconds` field refuses too: it was authored under that
    /// field's old meaning, and silently dropping it would half-load the
    /// author's intent — re-save it through the harness (which migrates) or
    /// rename the field.
    pub fn from_harness(text: &str) -> Option<PoseCurve> {
        let v = Value::parse(text)?;
        if v.get("format")?.as_str()? != "petramond-swing-animation" {
            return None;
        }
        if v.get("version")?.as_f64()? != 1.0 {
            return None;
        }
        if v.get("window_seconds").is_some() {
            return None;
        }
        let window_attack = window(&v, "window_attack")?;
        let window_mine = window(&v, "window_mine")?;
        let mut keys = Vec::new();
        let mut impact = None;
        for k in v.get("keys")?.as_array()? {
            let t = k.get("t")?.as_f64()? as f32;
            if !t.is_finite() || !(0.0..=1.0).contains(&t) {
                return None;
            }
            let origin = match k.get("origin") {
                None => [0.0; 3],
                Some(o) => numbers3(o)?,
            };
            // The impact is ONE instant of the swing: a second flagged key,
            // or the rest key at 0, is an authoring error the file refuses
            // whole rather than half-loads.
            match k.get("impact") {
                None => {}
                Some(flag) => {
                    if flag.as_bool()? && (impact.is_some() || t <= 0.0) {
                        return None;
                    }
                    if flag.as_bool()? {
                        impact = Some(t);
                    }
                }
            }
            keys.push(PoseKey {
                t,
                rotation: numbers3(k.get("rotation")?)?,
                translation: numbers3(k.get("translation")?)?,
                origin,
            });
        }
        if keys.len() < 2 {
            return None;
        }
        keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        if keys[0].t != 0.0 || keys.windows(2).any(|w| (w[1].t - w[0].t) < 1e-6) {
            return None;
        }
        Some(PoseCurve {
            keys,
            window_attack,
            window_mine,
            impact,
        })
    }

    /// The eased channel at `phase`.
    pub fn sample(&self, phase: f32) -> PoseSample {
        let (lo, hi, u) = bracket(&self.keys, |k| k.t, phase.clamp(0.0, 1.0));
        PoseSample {
            rotation: mix3(lo.rotation, hi.rotation, u),
            translation: mix3(lo.translation, hi.translation, u),
            origin: mix3(lo.origin, hi.origin, u),
        }
    }

    /// The authored ATTACK window (seconds), when the file carries one.
    pub fn window_attack(&self) -> Option<f32> {
        self.window_attack
    }

    /// The authored WORK window (seconds) — the mining loop and its break
    /// impacts — when the file carries one.
    pub fn window_mine(&self) -> Option<f32> {
        self.window_mine
    }

    /// The phase (`0 < t <= 1`) of the key the author flagged as the
    /// swing's IMPACT — the instant the motion lands — when the file marks
    /// one. What happens there is the consumer's business.
    pub fn impact(&self) -> Option<f32> {
        self.impact
    }
}

// ---- the body channel ------------------------------------------------------

/// One bone's `(rotation degrees, translation px)` channel value.
type BoneChannel = ([f32; 3], [f32; 3]);

#[derive(Clone, Debug, PartialEq)]
struct BodyKey {
    t: f32,
    /// `(index into bones, channel value)`.
    pose: Vec<(usize, BoneChannel)>,
}

/// One `petramond-player-animation` document: named rig bones, each Compose
/// or Replace, keyed on phase. A key poses a SUBSET of the bones — an absent
/// bone reads as zeros, and the interpolation still travels through it.
///
/// Read and DROPPED: `name`, `window_seconds` and `loop` — pacing is the
/// paired pose channel's (`window_attack`/`window_mine` on the item file);
/// the body plays at whatever window the swing does.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyCurve {
    /// `(bone, mode)` in authored APPLY ORDER (it matters when a parent and
    /// its child are both posed).
    bones: Vec<(String, BonePoseMode)>,
    keys: Vec<BodyKey>,
}

impl BodyCurve {
    /// Parse one harness document, whole. A key posing a bone the `bones`
    /// list does not declare is malformed: the list is the claim set, and a
    /// stray row would pose a bone the file never promised to own.
    pub fn from_harness(text: &str) -> Option<BodyCurve> {
        let v = Value::parse(text)?;
        if v.get("format")?.as_str()? != "petramond-player-animation" {
            return None;
        }
        if v.get("version")?.as_f64()? != 1.0 {
            return None;
        }
        let mut bones = Vec::new();
        for entry in v.get("bones")?.as_array()? {
            let name = entry.get("name")?.as_str()?;
            let mode = match entry.get("mode")?.as_str()? {
                "compose" => BonePoseMode::Compose,
                "replace" => BonePoseMode::Replace,
                _ => return None,
            };
            if name.is_empty() || bones.iter().any(|(n, _)| n == name) {
                return None;
            }
            bones.push((name.to_string(), mode));
        }
        if bones.is_empty() {
            return None;
        }
        let mut keys = Vec::new();
        for k in v.get("keys")?.as_array()? {
            let t = k.get("t")?.as_f64()? as f32;
            if !t.is_finite() || !(0.0..=1.0).contains(&t) {
                return None;
            }
            let mut pose = Vec::new();
            for (name, at) in bones.iter().map(|(n, _)| n).zip(0..) {
                let Some(ch) = k.get("pose")?.get(name) else {
                    continue;
                };
                pose.push((
                    at,
                    (
                        numbers3(ch.get("rotation")?)?,
                        numbers3(ch.get("translation")?)?,
                    ),
                ));
            }
            if k.get("pose")?.as_object()?.len() != pose.len() {
                return None;
            }
            keys.push(BodyKey { t, pose });
        }
        if keys.len() < 2 {
            return None;
        }
        keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        if keys[0].t != 0.0 || keys.windows(2).any(|w| (w[1].t - w[0].t) < 1e-6) {
            return None;
        }
        Some(BodyCurve { bones, keys })
    }

    /// The declared bones and their modes, in authored apply order. Resolve
    /// these once and sample per entry ([`sample_entry`]) on a hot path.
    ///
    /// [`sample_entry`]: Self::sample_entry
    pub fn entries(&self) -> &[(String, BonePoseMode)] {
        &self.bones
    }

    /// The eased `(rotation degrees, translation px)` of one declared bone
    /// (by its [`entries`](Self::entries) index) at `phase` — allocation-free.
    pub fn sample_entry(&self, entry: usize, phase: f32) -> ([f32; 3], [f32; 3]) {
        let (lo, hi, u) = bracket(&self.keys, |k| k.t, phase.clamp(0.0, 1.0));
        let channel = |key: &BodyKey| {
            key.pose
                .iter()
                .find(|(i, _)| *i == entry)
                .map(|(_, ch)| *ch)
                .unwrap_or_default()
        };
        let (a, b) = (channel(lo), channel(hi));
        (mix3(a.0, b.0, u), mix3(a.1, b.1, u))
    }

    /// The eased seam rows at `phase` — one row per declared bone, EVERY
    /// call (a zero Compose is a no-op and a zero Replace is rest, but a row
    /// that came and went between frames would flicker the claim itself).
    pub fn bones(&self, phase: f32) -> Vec<BonePoseData> {
        self.bones
            .iter()
            .enumerate()
            .map(|(at, (bone, mode))| {
                let (rotation, translation) = self.sample_entry(at, phase);
                BonePoseData {
                    bone: bone.clone(),
                    rotation,
                    translation,
                    mode: *mode,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid pose document builder: two keys with authored values.
    fn pose_doc(extra: &str, keys: &str) -> String {
        format!(
            "{{\"format\": \"petramond-swing-animation\", \"version\": 1{extra}, \
             \"keys\": [{keys}]}}"
        )
    }

    const REST_KEYS: &str = "{\"t\": 0, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}, \
         {\"t\": 1, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}";

    /// Whole-or-nothing: wrong format, wrong version, the RETIRED
    /// `window_seconds` field, a non-finite channel, too few keys, and an
    /// out-of-range time all refuse the WHOLE file — never a half-loaded
    /// curve. A present-but-broken window refuses too, while an absent one
    /// is simply the consumer's default.
    #[test]
    fn a_pose_document_parses_whole_or_not_at_all() {
        assert!(PoseCurve::from_harness(&pose_doc("", REST_KEYS)).is_some());
        assert!(PoseCurve::from_harness("{").is_none());
        assert!(PoseCurve::from_harness("{\"format\": \"other\"}").is_none());
        assert!(
            PoseCurve::from_harness(&pose_doc("", REST_KEYS).replace(": 1,", ": 9,")).is_none()
        );
        // The retired field was authored under its old meaning — re-save it
        // through the harness (which migrates) rather than half-load it.
        assert!(
            PoseCurve::from_harness(&pose_doc(", \"window_seconds\": 0.4", REST_KEYS)).is_none()
        );
        assert!(
            PoseCurve::from_harness(&pose_doc(", \"window_attack\": 0.0", REST_KEYS)).is_none()
        );
        assert!(
            PoseCurve::from_harness(&pose_doc(", \"window_attack\": 0.4", REST_KEYS)).is_some()
        );
        assert!(PoseCurve::from_harness(&pose_doc(
            "",
            "{\"t\": 0, \"rotation\": [0, 0, null], \"translation\": [0, 0, 0]}, \
             {\"t\": 1, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}"
        ))
        .is_none());
        assert!(PoseCurve::from_harness(&pose_doc(
            "",
            "{\"t\": 0, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}"
        ))
        .is_none());
        assert!(PoseCurve::from_harness(&pose_doc(
            "",
            "{\"t\": 0, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}, \
             {\"t\": 1.5, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}"
        ))
        .is_none());
    }

    /// The impact marker is ONE instant: a single flagged key answers its
    /// phase, an unflagged file answers none, and a second flag or a flag
    /// on the rest key refuses the whole file — a swing that lands twice,
    /// or before it moved, is an authoring error, never a half-loaded one.
    #[test]
    fn the_impact_marker_is_one_instant_or_the_file_refuses() {
        let keys = |a: &str, b: &str| {
            format!(
                "{{\"t\": 0, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]{a}}}, \
                 {{\"t\": 0.5, \"rotation\": [1, 0, 0], \"translation\": [0, 0, 0]{b}}}, \
                 {{\"t\": 1, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}}"
            )
        };
        let parse = |a, b| PoseCurve::from_harness(&pose_doc("", &keys(a, b)));
        assert_eq!(parse("", "").expect("unflagged parses").impact(), None);
        assert_eq!(
            parse("", ", \"impact\": true")
                .expect("one flag parses")
                .impact(),
            Some(0.5)
        );
        assert_eq!(
            parse("", ", \"impact\": false")
                .expect("a false flag is no flag")
                .impact(),
            None
        );
        assert!(
            parse(", \"impact\": true", "").is_none(),
            "the rest key cannot land"
        );
        assert!(
            parse(", \"impact\": true", ", \"impact\": true").is_none(),
            "two impacts refuse whole"
        );
        assert!(
            parse("", ", \"impact\": 1").is_none(),
            "a non-boolean flag is malformed"
        );
    }

    /// The sample at an authored key IS that key, full precision — a
    /// rounding step between the file and the playback would silently shift
    /// every phase of a curve — and two identical adjacent keys read as a
    /// FREEZE across their whole span, because there is nothing to
    /// interpolate toward. Past the last key the curve holds it.
    #[test]
    fn keys_read_back_exactly_and_duplicates_freeze() {
        let doc = pose_doc(
            "",
            "{\"t\": 0, \"rotation\": [0, 0, -1.5], \"translation\": [0, 0, 0]}, \
             {\"t\": 0.255186153, \"rotation\": [12.5, -51.5, -68.0], \
              \"translation\": [5.3175, 0, 0], \"origin\": [1, -6, 2]}, \
             {\"t\": 0.52, \"rotation\": [-18.5, 88.5, -79.0], \"translation\": [0, 0, -8.5]}, \
             {\"t\": 0.72, \"rotation\": [-18.5, 88.5, -79.0], \"translation\": [0, 0, -8.5]}, \
             {\"t\": 0.9, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}",
        );
        let curve = PoseCurve::from_harness(&doc).expect("a valid document");
        let at = |p: f32| curve.sample(p);
        assert_eq!(at(0.0).rotation, [0.0, 0.0, -1.5], "the authored rest");
        let key = at(0.255_186_14);
        assert_eq!(key.rotation, [12.5, -51.5, -68.0]);
        assert_eq!(key.translation, [5.3175, 0.0, 0.0]);
        assert_eq!(key.origin, [1.0, -6.0, 2.0]);
        for phase in [0.52, 0.6, 0.72] {
            assert_eq!(
                at(phase).rotation,
                [-18.5, 88.5, -79.0],
                "the duplicate pair freezes at {phase}"
            );
        }
        assert_eq!(at(1.0).rotation, [0.0; 3], "held past the last key");
    }

    /// A body document builder around one declared-bones list.
    fn body_doc(bones: &str, keys: &str) -> String {
        format!(
            "{{\"format\": \"petramond-player-animation\", \"version\": 1, \
             \"bones\": [{bones}], \"keys\": [{keys}]}}"
        )
    }

    /// The `bones` list is the claim set: a bad mode, a key posing an
    /// undeclared bone, a duplicate declaration, and an empty list all
    /// refuse WHOLE — a stray row would pose a bone the file never promised
    /// to own.
    #[test]
    fn a_body_document_refuses_outside_its_declared_claim_set() {
        let shoulder = "{\"name\": \"left_shoulder\", \"mode\": \"compose\"}";
        let posed = |bone: &str| {
            format!(
                "{{\"t\": 0, \"pose\": {{}}}}, {{\"t\": 1, \"pose\": \
                 {{\"{bone}\": {{\"rotation\": [1, 0, 0], \"translation\": [0, 0, 0]}}}}}}"
            )
        };
        assert!(BodyCurve::from_harness(&body_doc(shoulder, &posed("left_shoulder"))).is_some());
        assert!(BodyCurve::from_harness(&body_doc(shoulder, &posed("head"))).is_none());
        assert!(BodyCurve::from_harness(&body_doc(
            &shoulder.replace("compose", "smear"),
            &posed("left_shoulder")
        ))
        .is_none());
        assert!(BodyCurve::from_harness(&body_doc(
            &format!("{shoulder}, {shoulder}"),
            &posed("left_shoulder")
        ))
        .is_none());
        assert!(BodyCurve::from_harness(&body_doc("", &posed("left_shoulder"))).is_none());
        assert!(BodyCurve::from_harness("{").is_none());
    }

    /// A bone ABSENT from a key reads as zeros (the interpolation still
    /// travels through it), and EVERY declared bone gets a row at every
    /// phase — a row that came and went between frames would flicker the
    /// claim itself.
    #[test]
    fn absent_bones_read_as_zeros_and_every_declared_bone_gets_a_row() {
        let doc = body_doc(
            "{\"name\": \"left_shoulder\", \"mode\": \"compose\"}, \
             {\"name\": \"left_elbow\", \"mode\": \"replace\"}",
            "{\"t\": 0, \"pose\": {}}, \
             {\"t\": 0.5, \"pose\": {\"left_shoulder\": \
              {\"rotation\": [30, 0, -14], \"translation\": [0, 0, 0]}}}, \
             {\"t\": 1, \"pose\": {}}",
        );
        let body = BodyCurve::from_harness(&doc).expect("a valid document");
        assert_eq!(
            body.entries(),
            [
                ("left_shoulder".to_string(), BonePoseMode::Compose),
                ("left_elbow".to_string(), BonePoseMode::Replace),
            ]
        );
        let rows = body.bones(0.5);
        assert_eq!(rows.len(), 2, "every declared bone, every phase");
        assert_eq!(rows[0].rotation, [30.0, 0.0, -14.0]);
        assert_eq!(rows[1].rotation, [0.0; 3], "absent bone = authored zeros");
        assert_eq!(rows[1].mode, BonePoseMode::Replace);
        // Halfway INTO the posed key the shoulder is travelling, not parked.
        let mid = body.bones(0.25);
        assert!(mid[0].rotation[0] > 0.0 && mid[0].rotation[0] < 30.0);
    }
}
