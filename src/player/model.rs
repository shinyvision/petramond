use std::sync::LazyLock;

use petramond_world::bbmodel::Model;

use super::rigs::{self, Presenter};

pub const PLAYER_MODEL_SCALE: f32 = super::EYE / 28.0;

pub const PLAYER_HIP_PX: f32 = 12.0;

pub const PLAYER_HIP_HEIGHT: f32 = PLAYER_HIP_PX * PLAYER_MODEL_SCALE;

pub fn player_model() -> &'static Model {
    static EMPTY: LazyLock<Model> = LazyLock::new(Model::empty);
    rigs::presented(Presenter::Body).map_or(&EMPTY, |(_, rig)| &rig.model)
}

pub fn bone_id(name: &str) -> Option<u16> {
    player_model()
        .bone_named(name)
        .and_then(|i| u16::try_from(i).ok())
}
