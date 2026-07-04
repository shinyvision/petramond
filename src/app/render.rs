use super::{now_seconds, ui_snapshot, App};
use crate::audio::{SpatialListener, SpatialSoundSource};
use crate::game::presentation::MobPresentation;
use crate::mob::MobSoundCategory;
use crate::render::{HeldItemFrame, Renderer};

impl App {
    /// Draw the current frame. The host calls this once per [`update`](Self::update);
    /// the simulation tick itself runs inside `update`, not here.
    pub fn render(&mut self, renderer: &mut Renderer) {
        let now = now_seconds();
        // The hand animation advances by render time (not sim time); clamp so a long
        // idle gap before the first active frame can't jump a swing mid-flight.
        let dt = ((now - self.last_render) as f32).clamp(0.0, 0.1);
        self.last_render = now;
        let screen_size = renderer.screen_size();

        if self.renderer_world_clear_pending {
            renderer.clear_world_state();
            self.renderer_world_clear_pending = false;
        }

        let shell = self.shell_ui_snapshot(screen_size, self.pointer.cursor());

        let Some(game) = self.game.as_mut() else {
            self.audio.clear_spatial();
            self.spatial_sound_commands.clear();
            self.spatial_mob_positions.clear();
            self.mob_sound_events.clear();
            self.mob_sound_state.clear();
            renderer.set_crosshair_visible(false);
            renderer.set_hand_visible(false);
            renderer.update_uniforms(
                &self.shell_camera,
                [0.60, 0.82, 1.00],
                now as f32,
                false,
                None,
            );
            renderer.set_ui(ui_snapshot::build(
                None,
                self.screen,
                screen_size,
                self.pointer.cursor(),
                shell,
            ));
            renderer.render();
            return;
        };

        renderer.set_crosshair_visible(self.screen.gameplay_enabled());
        renderer.set_hand_visible(!matches!(self.screen, crate::app::AppScreen::Pause));

        let listener;
        {
            let frame = game.client_frame(now);
            listener = SpatialListener {
                pos: frame.camera.pos,
                right: frame.camera.right(),
            };
            renderer.update_uniforms(
                frame.camera,
                frame.environment.fog,
                frame.environment.time,
                frame.environment.underwater,
                Some(&frame.environment.shader_params),
            );
            renderer.set_selection(
                self.screen
                    .gameplay_enabled()
                    .then_some(frame.selection)
                    .flatten(),
            );
            let hand = std::mem::take(&mut self.hand);
            renderer.set_held_item(HeldItemFrame {
                item: frame.held_item.item,
                mining: frame.held_item.mining,
                broke_block: hand.broke,
                placed: hand.placed,
                swung: hand.swung,
                dt,
            });
        }
        // Build the neutral read snapshot, then bake it into render wire structs.
        {
            let current_tick = game.current_tick();
            let presentation = self.presentation.snapshot(game);
            renderer.set_break_overlay(presentation.break_overlay);
            self.spatial_mob_positions.clear();
            self.spatial_mob_positions.extend(
                presentation
                    .mobs
                    .iter()
                    .map(|m| (m.id, m.prev_pos.lerp(m.pos, presentation.tick_alpha))),
            );
            for command in self.spatial_sound_commands.drain(..) {
                match command {
                    crate::game::ModSpatialSoundCommand::PlayAt {
                        handle,
                        sound,
                        pos,
                        volume,
                        pitch,
                    } => self.audio.play_spatial(
                        handle,
                        sound,
                        SpatialSoundSource::Fixed(pos),
                        volume,
                        pitch,
                        listener,
                        pos,
                    ),
                    crate::game::ModSpatialSoundCommand::PlayOnMob {
                        handle,
                        sound,
                        mob_id,
                        volume,
                        pitch,
                        last_pos,
                    } => {
                        let initial = self
                            .spatial_mob_positions
                            .iter()
                            .find(|(id, _)| *id == mob_id)
                            .map(|(_, pos)| *pos)
                            .unwrap_or(last_pos);
                        self.audio.play_spatial(
                            handle,
                            sound,
                            SpatialSoundSource::Mob(mob_id),
                            volume,
                            pitch,
                            listener,
                            initial,
                        );
                    }
                    crate::game::ModSpatialSoundCommand::Stop { handle } => {
                        self.audio.stop_spatial(handle);
                    }
                }
            }
            play_pending_mob_sound_events(
                &mut self.audio,
                &mut self.mob_sound_events,
                &mut self.next_mob_sound_handle,
                listener,
                &self.spatial_mob_positions,
            );
            if self.screen.gameplay_enabled() {
                tick_idle_mob_sounds(
                    &mut self.audio,
                    &mut self.mob_sound_state,
                    &mut self.next_mob_sound_handle,
                    listener,
                    presentation.mobs,
                    &self.spatial_mob_positions,
                    current_tick,
                );
            }
            self.audio
                .update_spatial(listener, &self.spatial_mob_positions);
            self.scene.bake(&presentation);
        }
        self.scene.upload(renderer);
        renderer.set_ui(ui_snapshot::build(
            Some(game),
            self.screen,
            screen_size,
            self.pointer.cursor(),
            shell,
        ));

        {
            let mut terrain = game.terrain_render_handoff();
            renderer.sync_meshes(&mut terrain);
        }
        renderer.render();
    }
}

fn play_pending_mob_sound_events(
    audio: &mut crate::audio::Audio,
    events: &mut Vec<crate::game::MobSoundEvent>,
    next_handle: &mut u64,
    listener: SpatialListener,
    positions: &[(u64, crate::mathh::Vec3)],
) {
    for event in events.drain(..) {
        let Some(spec) = crate::mob::def(event.kind).sound_for(event.category) else {
            continue;
        };
        let initial = mob_position(positions, event.mob_id).unwrap_or(event.pos);
        play_mob_sound(
            audio,
            next_handle,
            spec.sound,
            event.mob_id,
            listener,
            initial,
        );
    }
}

fn tick_idle_mob_sounds(
    audio: &mut crate::audio::Audio,
    states: &mut std::collections::HashMap<u64, super::MobSoundState>,
    next_handle: &mut u64,
    listener: SpatialListener,
    mobs: &[MobPresentation],
    positions: &[(u64, crate::mathh::Vec3)],
    current_tick: u64,
) {
    for mob in mobs {
        if mob.dead {
            continue;
        }
        let Some(spec) = crate::mob::def(mob.kind).sound_for(MobSoundCategory::Idle) else {
            continue;
        };
        let state = states
            .entry(mob.id)
            .or_insert_with(|| super::MobSoundState {
                next_idle_tick: current_tick.saturating_add(idle_delay_ticks(mob.id, 0, spec)),
                sequence: 0,
            });
        if current_tick < state.next_idle_tick {
            continue;
        }
        let initial = mob_position(positions, mob.id).unwrap_or(mob.pos);
        play_mob_sound(audio, next_handle, spec.sound, mob.id, listener, initial);
        state.sequence = state.sequence.wrapping_add(1);
        state.next_idle_tick =
            current_tick.saturating_add(idle_delay_ticks(mob.id, state.sequence, spec));
    }
    states.retain(|id, _| mobs.iter().any(|m| m.id == *id && !m.dead));
}

fn play_mob_sound(
    audio: &mut crate::audio::Audio,
    next_handle: &mut u64,
    sound: crate::audio::Sound,
    mob_id: u64,
    listener: SpatialListener,
    initial: crate::mathh::Vec3,
) {
    audio.play_spatial_randomized(
        alloc_mob_sound_handle(next_handle),
        sound,
        SpatialSoundSource::Mob(mob_id),
        listener,
        initial,
    );
}

fn alloc_mob_sound_handle(next: &mut u64) -> u64 {
    let handle = (*next).max(super::MOB_SOUND_HANDLE_START);
    *next = handle.wrapping_add(1).max(super::MOB_SOUND_HANDLE_START);
    handle
}

fn mob_position(
    positions: &[(u64, crate::mathh::Vec3)],
    mob_id: u64,
) -> Option<crate::mathh::Vec3> {
    positions
        .iter()
        .find(|(id, _)| *id == mob_id)
        .map(|(_, pos)| *pos)
}

fn idle_delay_ticks(mob_id: u64, sequence: u64, spec: &crate::mob::MobSoundSpec) -> u64 {
    let base = spec.tick_interval.unwrap_or(1) as u64;
    let variance = spec.tick_interval_variance as u64;
    let lo = base.saturating_sub(variance).max(1);
    let hi = base.saturating_add(variance).max(lo);
    lo + mix64(mob_id ^ sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15)) % (hi - lo + 1)
}

fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}
