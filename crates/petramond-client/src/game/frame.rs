use petramond_render::camera::Camera;
use petramond_world::block_state::HeldBlockState;
use petramond_world::item::ItemType;
use petramond_world::selection::SelectionShape;

use super::{Game, GameEnvironment};
use crate::animation::BoneOffset;

pub struct ClientFrame<'a> {
    pub camera: &'a Camera,
    pub environment: GameEnvironment,
    pub selection: Option<SelectionShape>,
    pub held_item: ClientHeldItem,
    pub off_hand_item: ClientHeldItem,
    pub animator: petramond::player::AnimatorClaims,
}

impl ClientFrame<'_> {
    pub fn listener(&self) -> petramond_audio::SpatialListener {
        petramond_audio::SpatialListener {
            pos: self.camera.pos,
            right: self.camera.right(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClientHeldItem {
    pub item: Option<ItemType>,
    pub display: Option<ItemType>,
    pub variant: petramond_world::item::VariantId,
    pub block_state: HeldBlockState,
    pub mining: bool,
    pub eating: Option<f32>,
    pub pose_target: Option<mod_api::HeldPose>,
}

pub fn render_held_pose(pose: mod_api::HeldPose) -> petramond_render::HeldPose {
    let view = |p: mod_api::HeldPoseData| petramond_world::block_model::DisplayTransform {
        rotation: p.rotation,
        translation: p.translation,
        ..Default::default()
    };
    petramond_render::HeldPose {
        first_person: view(pose.first_person),
        third_person: view(pose.third_person),
    }
}

pub fn render_bone_offsets(poses: &[petramond::player::BonePose], out: &mut Vec<BoneOffset>) {
    let bones = petramond::player::model::player_model().bones().len();
    out.extend(
        poses
            .iter()
            .filter(|p| (p.bone as usize) < bones)
            .map(|p| BoneOffset {
                bone: p.bone as usize,
                rotation: p.rotation,
                translation: p.translation,
                hold: p.hold,
            }),
    );
}

impl Game {
    pub fn client_frame(&self, now: f64) -> ClientFrame<'_> {
        let view = &self.replica.self_view;
        let mining = view.mining.is_some();
        let eating = self.eating_progress();
        let (eat_main, eat_off) = if view.eating_off_hand {
            (None, eating)
        } else {
            (eating, None)
        };
        let (pose_main, pose_off) = self
            .client_mods
            .local_held_poses((view.held_pose_main, view.held_pose_off));
        let [display_main, display_off] = self.client_mods.local_held_displays(view.held_display);
        let animator = self.client_mods.local_animator(&view.animator);
        let mut environment = self.environment(now);
        environment.shader_params = self.presented_shader_params(&environment.shader_params);
        ClientFrame {
            camera: self.render_camera(),
            environment,
            selection: self.local.look.map(|h| h.outline),
            held_item: ClientHeldItem {
                item: view.inventory.selected().map(|s| s.item),
                display: display_main,
                variant: view
                    .inventory
                    .selected()
                    .map(|s| s.variant)
                    .unwrap_or_default(),
                block_state: self.held_block_state(),
                mining,
                eating: eat_main,
                pose_target: pose_main,
            },
            off_hand_item: ClientHeldItem {
                item: view.inventory.off_hand().map(|s| s.item),
                display: display_off,
                variant: view
                    .inventory
                    .off_hand()
                    .map(|s| s.variant)
                    .unwrap_or_default(),
                block_state: Default::default(),
                mining: false,
                eating: eat_off,
                pose_target: pose_off,
            },
            animator,
        }
    }
}
