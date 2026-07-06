//! The player's compiled entity model (`assets/models/entities/player.bbmodel`),
//! precached once like the mob models (pack-overridable source, `.llmob` cache)
//! and borrowed by the renderer for the third-person body bake.

use std::sync::LazyLock;

use crate::bbmodel::Model;

/// Pack-relative source path of the player body model.
const PLAYER_MODEL_PATH: &str = "models/entities/player.bbmodel";

/// Pixels → blocks for the player body. The model is authored 32 px tall with the
/// eye line mid-head at 28 px; this scale puts that line at the physics eye height
/// (`player::EYE`), making the rendered body ~1.85 blocks — a hair over the 1.8
/// collision box, matching how the reference model overhangs its hitbox.
pub const PLAYER_MODEL_SCALE: f32 = super::EYE / 28.0;

static PLAYER_MODEL: LazyLock<Model> = LazyLock::new(|| {
    let Some((src, _)) = crate::assets::read_bytes(PLAYER_MODEL_PATH) else {
        log::error!("player model '{PLAYER_MODEL_PATH}' not found in the asset roots");
        return Model::empty();
    };
    crate::asset_cache::load_or_compile::<Model>("player", &src).unwrap_or_else(|e| {
        log::error!("player model precache failed: {e}");
        Model::empty()
    })
});

/// The precached player [`Model`], borrowed for the process lifetime.
pub fn player_model() -> &'static Model {
    &PLAYER_MODEL
}
