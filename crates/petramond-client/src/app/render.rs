use super::{now_seconds, ui_snapshot, App};
use crate::game::presentation::MobPresentation;
use petramond::mob::MobSoundCategory;
use petramond_audio::{SpatialListener, SpatialSoundSource};
use petramond_render::{DocumentUiFrame, HeldItemFrame, Renderer, UiFrame};

impl App {
    /// Draw the current frame. The host calls this once per [`update`](Self::update);
    /// the simulation tick itself runs inside `update`, not here. Returns `false`
    /// only when a resize or screen transition made the solved UI stamp stale;
    /// the host then schedules an immediate update instead of presenting it.
    pub fn render(&mut self, renderer: &mut Renderer) -> bool {
        let now = now_seconds();
        // The hand animation advances by render time (not sim time); clamp so a long
        // idle gap before the first active frame can't jump a swing mid-flight.
        let dt = ((now - self.last_render) as f32).clamp(0.0, 0.1);
        self.last_render = now;
        self.push_renderer_options(renderer);
        let viewport = renderer.ui_viewport();
        let screen_size = viewport.size;
        self.ui.set_viewport_generation(viewport.generation);

        if self.renderer_world_clear_pending {
            renderer.clear_world_state();
            self.renderer_world_clear_pending = false;
        }

        // Document-backed screens draw the frame [`App::update`] already
        // built (`drive_doc_ui`/`drive_doc_menu`); the hotbar HUD document is
        // presentation-only, so it runs its (input-free) frame here.
        let mut doc_kind = self.doc_ui_kind();
        if doc_kind.is_none() && self.doc_hud_active() {
            let kind = petramond_world::gui_state::GuiKind::Hotbar;
            self.ui.ensure_active(kind);
            if let Some(game) = self.game.as_ref() {
                let active = game.menu_read_model().inventory.active_slot();
                self.ui
                    .state_mut()
                    .set("active_slot", petramond_ui::UiValue::I32(active as i32));
            }
            self.ui.frame(kind, screen_size, now, None);
            doc_kind = Some(kind);
        }
        if let Some(kind) = doc_kind {
            if self.ui.frame_stamp() != Some((kind, viewport)) {
                return false;
            }
        }
        let document_viewport = self.ui.frame_stamp().map(|(_, viewport)| viewport);
        if doc_kind.is_some() {
            if matches!(
                self.screen,
                crate::app::AppScreen::Game | crate::app::AppScreen::Chat
            ) && self.game.is_some()
            {
                self.chat.draw(
                    self.ui.draw_mut(),
                    screen_size,
                    self.screen == crate::app::AppScreen::Chat,
                    now,
                );
            }
        } else {
            self.ui.deactivate();
        }
        self.compose_document_ui(doc_kind.is_some());
        self.compose_client_overlays(screen_size);
        let doc_slots = doc_kind.map(|_| self.ui.doc_slots());
        let doc_hooks = doc_kind.map(|_| self.ui.doc_hooks());
        let menu_drag_preview = self
            .screen
            .ui_open()
            .then(|| self.ui.menu_drag_preview())
            .flatten();

        let Some(game) = self.game.as_mut() else {
            // No session, no health bar: a fresh world must never wiggle off a
            // comparison against the previous session's last health.
            self.prev_heart_health = None;
            self.heart_wiggle = None;
            self.audio.clear_spatial();
            // The mood eases back to the untouched image once no session
            // exists (its owner died with the world).
            renderer.set_mood([0.0, 0.0], dt);
            self.spatial_sound_commands.clear();
            self.spatial_mob_positions.clear();
            self.mob_sound_events.clear();
            self.world_sound_cues.clear();
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
            let mut ui = ui_snapshot::build(None, self.screen, self.pointer.cursor(), None);
            if let Some(kind) = doc_kind {
                ui.kind = kind;
            }
            let document = doc_kind.map(|kind| DocumentUiFrame {
                viewport: document_viewport.expect("document frame was validated above"),
                kind,
                draw: &self.composed_doc,
                images: &self.composed_doc_images,
                slots: doc_slots.as_deref().map(Vec::as_slice).unwrap_or(&[]),
                hooks: doc_hooks.as_deref().map(Vec::as_slice).unwrap_or(&[]),
            });
            if !renderer.prepare_ui_frame(UiFrame {
                viewport,
                document,
                content: &ui,
                client_overlays: &self.client_overlay_images,
                client_overlay_dim: self.screen.client_canvas_open(),
            }) {
                return false;
            }
            renderer.render();
            return true;
        };

        renderer.set_crosshair_visible(self.screen.gameplay_enabled());
        self.sleep_interact_hand_t = (self.sleep_interact_hand_t - dt).max(0.0);
        let hand_visible = match self.screen {
            crate::app::AppScreen::Pause | crate::app::AppScreen::Dead => false,
            crate::app::AppScreen::Sleeping => {
                self.sleep_interact_hand_t > 0.0 && !game.third_person_enabled()
            }
            // Third person shows the whole body instead of the floating hand.
            _ => !game.third_person_enabled(),
        };
        renderer.set_hand_visible(hand_visible);

        // The hurt shake: a short decaying jitter on the camera look and the
        // hand's screen position. Presentation-only — the sim camera state is
        // untouched; a clone carries the offset into the uniforms.
        self.hurt_shake_t = (self.hurt_shake_t - dt).max(0.0);
        let shake = hurt_shake(self.hurt_shake_t, now);
        renderer.set_hand_shake(shake.hand);

        let listener;
        {
            let frame = game.client_frame(now);
            listener = SpatialListener {
                pos: frame.camera.pos,
                right: frame.camera.right(),
            };
            let mut cam = frame.camera.clone();
            cam.yaw += shake.yaw;
            cam.pitch += shake.pitch;
            renderer.update_uniforms(
                &cam,
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
                variant: frame.held_item.variant,
                block_state: frame.held_item.block_state,
                mining: frame.held_item.mining,
                broke_block: hand.broke,
                placed: hand.placed,
                swung: hand.swung,
                eating: frame.held_item.eating,
                pose_target: frame
                    .held_item
                    .pose_target
                    .map(crate::game::render_held_pose),
                swing_claim: frame.held_item.motions.contains(mod_api::HandMotion::Swing),
                jab_claim: frame.held_item.motions.contains(mod_api::HandMotion::Jab),
                bob: frame.held_item.bob,
                dt,
            });
            // The OFF hand: its own item + jab/eat channels. Mining, breaks,
            // and attack swings are main-hand actions by definition.
            renderer.set_off_hand_item(HeldItemFrame {
                item: frame.off_hand_item.item,
                variant: frame.off_hand_item.variant,
                block_state: frame.off_hand_item.block_state,
                mining: false,
                broke_block: false,
                placed: hand.placed_off,
                swung: false,
                eating: frame.off_hand_item.eating,
                pose_target: frame
                    .off_hand_item
                    .pose_target
                    .map(crate::game::render_held_pose),
                swing_claim: frame
                    .off_hand_item
                    .motions
                    .contains(mod_api::HandMotion::Swing),
                jab_claim: frame
                    .off_hand_item
                    .motions
                    .contains(mod_api::HandMotion::Jab),
                bob: frame.off_hand_item.bob,
                dt,
            });
        }
        // Build the neutral read snapshot, then bake it into render wire structs.
        {
            let current_tick = game.current_tick();
            // The same hourly-wrapped clock `GameEnvironment::time` carries, so
            // ambient volumes animate on the exact clock looping emitters do.
            // The renderer's own view volume, published by `update_uniforms`
            // above (shake included) and unchanged until this frame draws — so
            // gathers cull against exactly what will be rasterized.
            let view = renderer.view_volume();
            let presentation = self
                .presentation
                .snapshot(game, (now % 3600.0) as f32, &view);
            renderer.set_break_overlays(presentation.break_overlays);
            self.spatial_mob_positions.clear();
            self.spatial_mob_positions.extend(
                presentation
                    .mobs
                    .iter()
                    .map(|m| (m.id, m.prev_pos.lerp(m.pos, presentation.tick_alpha))),
            );
            for command in self.spatial_sound_commands.drain(..) {
                match command {
                    crate::game::SpatialSoundCommand::PlayAt {
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
                    crate::game::SpatialSoundCommand::PlayOnMob {
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
                    crate::game::SpatialSoundCommand::Stop { handle } => {
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
            // Positional world-event one-shots (place/break/door/chest/foreign
            // pickup): fire-and-forget spatial plays off the same client-local
            // wrapping handle pool the mob sounds use.
            for (sound, pos) in self.world_sound_cues.drain(..) {
                self.audio.play_spatial_randomized(
                    alloc_mob_sound_handle(&mut self.next_mob_sound_handle),
                    sound,
                    SpatialSoundSource::Fixed(pos),
                    listener,
                    pos,
                );
            }
            // Footsteps ride the same live world clock as the mob idle
            // cadence below, and for the same reason.
            tick_footstep_sounds(
                &mut self.audio,
                &mut self.footstep_next_tick,
                &mut self.next_mob_sound_handle,
                listener,
                presentation.footsteps,
                current_tick,
            );
            // Idle cadence follows the live world clock, not gameplay input:
            // menus and multiplayer pause can keep that clock moving.
            tick_idle_mob_sounds(
                &mut self.audio,
                &mut self.mob_sound_state,
                &mut self.next_mob_sound_handle,
                listener,
                presentation.mobs,
                &self.spatial_mob_positions,
                current_tick,
            );
            self.audio
                .update_spatial(listener, &self.spatial_mob_positions);
            // Client-mod looping ambience (rain beds, wind): sync desired
            // gains, ease audio-side.
            game.client_mod_sound_loops(&mut self.loop_gain_scratch);
            self.audio.update_gain_loops(&self.loop_gain_scratch, dt);
            renderer.set_mood(game.client_mod_mood(), dt);
            // The hurt vignette envelope doubles as the body's red hurt flash.
            self.scene.bake(&presentation, shake.flash);
        }
        self.scene.upload(renderer);
        let drag_preview = menu_drag_preview
            .as_ref()
            .map(|(slots, button)| (slots.as_slice(), *button));
        let mut ui =
            ui_snapshot::build(Some(game), self.screen, self.pointer.cursor(), drag_preview);
        ui.craft_recipes
            .extend(self.crafting_browser.views().cloned());
        ui.craft_tip = self.crafting_browser.tip_view().cloned();
        if let Some(kind) = doc_kind {
            ui.kind = kind;
        }
        ui.hurt_flash = shake.flash;
        ui.heart_wiggle = heart_wiggle_frame(
            &mut self.prev_heart_health,
            &mut self.heart_wiggle,
            ui.health,
            now,
        );
        let document = doc_kind.map(|kind| DocumentUiFrame {
            viewport: document_viewport.expect("document frame was validated above"),
            kind,
            draw: &self.composed_doc,
            images: &self.composed_doc_images,
            slots: doc_slots.as_deref().map(Vec::as_slice).unwrap_or(&[]),
            hooks: doc_hooks.as_deref().map(Vec::as_slice).unwrap_or(&[]),
        });
        if !renderer.prepare_ui_frame(UiFrame {
            viewport,
            document,
            content: &ui,
            client_overlays: &self.client_overlay_images,
            client_overlay_dim: self.screen.client_canvas_open(),
        }) {
            return false;
        }

        let terrain_busy = {
            let mut terrain = game.terrain_render_handoff();
            renderer.sync_meshes(&mut terrain);
            terrain.is_streaming()
        };
        renderer.render();
        self.heap_reclaim
            .frame(terrain_busy || renderer.terrain_uploads_pending());
        true
    }
}

/// Frame-side heart-wiggle bookkeeping: ANY change in the HUD health — a
/// regen heal, fall damage, a mob hit, whatever the source — starts a
/// [`HEART_WIGGLE_SECS`](super::HEART_WIGGLE_SECS) wall-clock wiggle on
/// exactly the hearts whose half-heart points changed. Returns this frame's
/// snapshot payload (`(lo, hi, seconds into the burst)`), or `None` when
/// nothing wiggles. A free function over the two state fields so it composes
/// with the long-lived `self.game` borrow in `render`.
fn heart_wiggle_frame(
    prev_health: &mut Option<i32>,
    wiggle: &mut Option<super::HeartWiggle>,
    health: Option<petramond_world::gui_state::HealthView>,
    now: f64,
) -> Option<(i32, i32, f32)> {
    let current = health.map(|h| h.current);
    // Both sides must exist: entering/leaving spectator (or the bar first
    // appearing at world join) is not a heal.
    if let (Some(prev), Some(cur)) = (*prev_health, current) {
        if cur != prev {
            *wiggle = Some(super::HeartWiggle {
                lo: cur.min(prev),
                hi: cur.max(prev),
                started: now,
            });
        }
    }
    *prev_health = current;
    let w = (*wiggle)?;
    let t = now - w.started;
    if t >= super::HEART_WIGGLE_SECS {
        *wiggle = None;
        return None;
    }
    Some((w.lo, w.hi, t as f32))
}

/// The hurt-shake offsets for this frame: camera look jitter (radians), a hand
/// screen offset (NDC), and the red edge-vignette strength. Two incommensurate
/// frequencies so the motion reads as a tremble, not a metronome; the squared
/// envelope front-loads the kick and dies smoothly. Punchy enough that a hit
/// is unmistakable, short enough that it never turns into a wobble.
struct HurtShake {
    yaw: f32,
    pitch: f32,
    hand: [f32; 2],
    /// Red edge-vignette strength `[0, 1]` (linear envelope — it should linger
    /// a touch longer than the motion).
    flash: f32,
}

fn hurt_shake(remaining: f32, now: f64) -> HurtShake {
    if remaining <= 0.0 {
        return HurtShake {
            yaw: 0.0,
            pitch: 0.0,
            hand: [0.0, 0.0],
            flash: 0.0,
        };
    }
    let envelope = (remaining / super::HURT_SHAKE_SECS).clamp(0.0, 1.0);
    let amp = envelope * envelope;
    let t = now as f32;
    let (a, b) = ((t * 71.0).sin(), (t * 53.0).cos());
    HurtShake {
        yaw: 0.011 * amp * a,
        pitch: 0.008 * amp * b,
        hand: [0.032 * amp * b, 0.026 * amp * a],
        flash: envelope,
    }
}

fn play_pending_mob_sound_events(
    audio: &mut petramond_audio::Audio,
    events: &mut Vec<crate::game::MobSoundEvent>,
    next_handle: &mut u64,
    listener: SpatialListener,
    positions: &[(u64, petramond_math::math::Vec3)],
) {
    for event in events.drain(..) {
        let Some(spec) = petramond::mob::def(event.kind).sound_for(event.category) else {
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

/// Ticks between one body's footsteps, walking and sprinting. The gait comes
/// from presentation (which sees the body's real speed); the cadence is here.
const FOOTSTEP_INTERVAL_TICKS: u64 = 10;
const SPRINT_FOOTSTEP_INTERVAL_TICKS: u64 = 7;

/// Sound one footstep per walking body whose cadence is due.
///
/// The presentation already decided WHO is walking and on WHAT (it needs the
/// world to answer that); this owns only the cadence and the play. A body first
/// seen walking steps IMMEDIATELY — its entry is seeded due — so movement is
/// heard the moment it starts rather than up to half a second later.
///
/// The map is keyed on the body, not the sound, so a player who pauses for a
/// step or two resumes ON the same cadence instead of retriggering; entries die
/// with the bodies (`footsteps` lists standing players too, which is what makes
/// that retire exact without a second list).
pub(super) fn tick_footstep_sounds(
    audio: &mut petramond_audio::Audio,
    next_tick: &mut std::collections::HashMap<u64, u64>,
    next_handle: &mut u64,
    listener: SpatialListener,
    footsteps: &[crate::game::presentation::FootstepSource],
    current_tick: u64,
) {
    for step in footsteps {
        let due = next_tick.entry(step.id).or_insert(current_tick);
        let Some(ground) = step.ground else {
            continue;
        };
        if current_tick < *due {
            continue;
        }
        *due = current_tick.saturating_add(if step.sprinting {
            SPRINT_FOOTSTEP_INTERVAL_TICKS
        } else {
            FOOTSTEP_INTERVAL_TICKS
        });
        let Some(sound) = ground.sound(petramond_world::block::BlockSoundAction::Step) else {
            continue;
        };
        // Fire-and-forget at the FEET, off the same wrapping handle pool the
        // world one-shots use, so a remote's steps arrive from their body.
        audio.play_spatial_randomized(
            alloc_mob_sound_handle(next_handle),
            sound,
            SpatialSoundSource::Fixed(step.pos),
            listener,
            step.pos,
        );
    }
    next_tick.retain(|id, _| footsteps.iter().any(|s| s.id == *id));
}

pub(super) fn tick_idle_mob_sounds(
    audio: &mut petramond_audio::Audio,
    states: &mut std::collections::HashMap<u64, super::MobSoundState>,
    next_handle: &mut u64,
    listener: SpatialListener,
    mobs: &[MobPresentation],
    positions: &[(u64, petramond_math::math::Vec3)],
    current_tick: u64,
) {
    for mob in mobs {
        if mob.dead {
            continue;
        }
        let Some(spec) = petramond::mob::def(mob.kind).sound_for(MobSoundCategory::Idle) else {
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
    audio: &mut petramond_audio::Audio,
    next_handle: &mut u64,
    sound: petramond_world::sound_registry::Sound,
    mob_id: u64,
    listener: SpatialListener,
    initial: petramond_math::math::Vec3,
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
    positions: &[(u64, petramond_math::math::Vec3)],
    mob_id: u64,
) -> Option<petramond_math::math::Vec3> {
    positions
        .iter()
        .find(|(id, _)| *id == mob_id)
        .map(|(_, pos)| *pos)
}

fn idle_delay_ticks(mob_id: u64, sequence: u64, spec: &petramond::mob::MobSoundSpec) -> u64 {
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
