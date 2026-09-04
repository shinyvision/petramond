//! The player's compiled entity model (`assets/models/entities/player.bbmodel`),
//! precached once like the mob models (pack-overridable source, `.llmob` cache)
//! and borrowed by the renderer for the third-person body bake.

use std::sync::LazyLock;

use petramond_world::bbmodel::Model;

/// Pack-relative source path of the player body model.
const PLAYER_MODEL_PATH: &str = "models/entities/player.bbmodel";

/// Pixels → blocks for the player body. The model is authored 32 px tall with the
/// eye line mid-head at 28 px; this scale puts that line at the physics eye height
/// (`player::EYE`), making the rendered body ~1.85 blocks — a hair over the 1.8
/// collision box, matching how the reference model overhangs its hitbox.
pub const PLAYER_MODEL_SCALE: f32 = super::EYE / 28.0;

/// The authored hip pivot of the player body (the `leftLeg`/`rightLeg` bone
/// origins), in model pixels above the feet.
pub const PLAYER_HIP_PX: f32 = 12.0;

/// The hip pivot in blocks: the point of a rider that actually rests on a
/// seat, and the point a seated body leans about. Both the seat projection
/// (`mob::riding::seat_world_pos`) and the renderer's seated lean read THIS,
/// so the seat and the body can never disagree about where the hips are.
pub const PLAYER_HIP_HEIGHT: f32 = PLAYER_HIP_PX * PLAYER_MODEL_SCALE;

static PLAYER_MODEL: LazyLock<Model> = LazyLock::new(|| {
    let Some((src, _)) = petramond_world::assets::read_bytes(PLAYER_MODEL_PATH) else {
        log::error!("player model '{PLAYER_MODEL_PATH}' not found in the asset roots");
        return Model::empty();
    };
    petramond_world::asset_cache::load_or_compile::<Model>("player", &src).unwrap_or_else(|e| {
        log::error!("player model precache failed: {e}");
        Model::empty()
    })
});

/// The precached player [`Model`], borrowed for the process lifetime.
pub fn player_model() -> &'static Model {
    &PLAYER_MODEL
}

/// Resolve a rig bone NAME to the compact id every runtime path carries.
///
/// The mod ABI speaks names, because a pack should not be writing indices into
/// a rig it does not own. Everything BELOW that boundary — the per-mod claim,
/// the wire row, the render instance — carries this id instead, so a bone
/// offset costs no allocation and no string comparison per tick.
///
/// The id is the bone's index in this process's player rig, and both mirrors
/// resolve against the same `player.bbmodel` from the same asset roots. A
/// client whose asset set puts a different rig behind that path simply drops
/// the offsets it cannot place, exactly as an unknown name does here.
pub fn bone_id(name: &str) -> Option<u16> {
    player_model()
        .bone_named(name)
        .and_then(|i| u16::try_from(i).ok())
}
