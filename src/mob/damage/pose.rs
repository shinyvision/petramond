use glam::Mat4;
use petramond_world::bbmodel::clips;

use crate::mob::{model, Instance};

#[cfg(test)]
mod tests;

impl Instance {
    pub(super) fn death_pose(&self) -> Vec<Mat4> {
        let model = model(self.kind);
        let base = if self.moving {
            model.animation(clips::WALK)
        } else {
            self.idle_anim
                .and_then(|i| model.idle_animation(i as usize))
        };
        let mut layers = Vec::with_capacity(self.active_anims().len() + 2);
        layers.extend(base.map(|a| (a, self.anim_time, 1.0)));
        layers.extend(
            self.active_anims()
                .iter()
                .filter_map(|layer| model.animation(&layer.name).map(|a| (a, layer.phase, 1.0))),
        );
        let posed = model.pose_mob_layers(
            &mut layers,
            self.head_yaw,
            self.head_pitch,
            self.hurt_flash(1.0),
        );
        posed
            .into_iter()
            .zip(model.rest_pose())
            .map(|(posed, rest)| posed * rest.inverse())
            .collect()
    }
}
