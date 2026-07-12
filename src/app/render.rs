use super::{now_seconds, ui_snapshot, App};
use crate::audio::{SpatialListener, SpatialSoundSource};
use crate::game::presentation::MobPresentation;
use crate::mob::MobSoundCategory;
use crate::render::{DocumentUiFrame, HeldItemFrame, Renderer, UiFrame};

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
            let kind = crate::gui::GuiKind::Hotbar;
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

        let Some(game) = self.game.as_mut() else {
            // No session, no health bar: a fresh world must never wiggle off a
            // comparison against the previous session's last health.
            self.prev_heart_health = None;
            self.heart_wiggle = None;
            self.audio.clear_spatial();
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
            let mut ui = ui_snapshot::build(None, self.screen, self.pointer.cursor());
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
                block_state: frame.held_item.block_state,
                mining: frame.held_item.mining,
                broke_block: hand.broke,
                placed: hand.placed,
                swung: hand.swung,
                eating: frame.held_item.eating,
                dt,
            });
        }
        // Build the neutral read snapshot, then bake it into render wire structs.
        {
            let current_tick = game.current_tick();
            let presentation = self.presentation.snapshot(game);
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
            // The hurt vignette envelope doubles as the body's red hurt flash.
            self.scene.bake(&presentation, shake.flash);
        }
        self.scene.upload(renderer);
        let mut ui = ui_snapshot::build(Some(game), self.screen, self.pointer.cursor());
        ui.craft_recipes
            .extend(self.crafting_browser.views().cloned());
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

        {
            let mut terrain = game.terrain_render_handoff();
            renderer.sync_meshes(&mut terrain);
        }
        renderer.render();
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
    health: Option<crate::gui::HealthView>,
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
