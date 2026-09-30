use glam::{Mat4, Quat};

use super::{clips, Animation, Model};

impl Model {
    /// Composes mob animation layers, hurt recoil and the unclaimed share of head look.
    /// Used by both live rendering and the simulation's pose at death.
    pub fn pose_mob_layers<'a>(
        &'a self,
        layers: &mut Vec<(&'a Animation, f32, f32)>,
        head_yaw: f32,
        head_pitch: f32,
        hurt: f32,
    ) -> Vec<Mat4> {
        let looking_head = self.head_bone().map(|hb| {
            let owned: f32 = layers
                .iter()
                .filter(|(a, _, _)| a.affects_bone(hb))
                .map(|(_, _, weight)| weight.clamp(0.0, 1.0))
                .sum();
            (hb, 1.0 - owned.min(1.0))
        });
        if hurt > 0.001 {
            if let Some(clip) = self.animation(clips::HURT) {
                layers.push((
                    clip,
                    (1.0 - hurt.clamp(0.0, 1.0)) * clip.length,
                    hurt.clamp(0.0, 1.0),
                ));
            }
        }
        let mut pose = if layers.is_empty() {
            self.rest_pose()
        } else {
            self.pose_layers(layers)
        };
        if let Some((hb, free)) = looking_head.filter(|(_, free)| *free > 0.001) {
            let parent = self.bones[hb]
                .parent
                .map(|p| pose[p].to_scale_rotation_translation().1)
                .unwrap_or(Quat::IDENTITY);
            let look = Quat::IDENTITY.slerp(
                Quat::from_rotation_y(head_yaw) * Quat::from_rotation_x(head_pitch),
                free,
            );
            self.apply_bone_rotation(&mut pose, hb, parent * look * parent.conjugate());
        }
        pose
    }
}
