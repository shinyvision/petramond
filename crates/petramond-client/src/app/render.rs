use super::client_audio::WorldAudioFrame;
use super::{now_seconds, ui_snapshot, App};
use petramond_audio::SpatialListener;
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
        self.apply_graphics(renderer);
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
            if let Some(game) = self.game.as_mut() {
                let active = game.menu_read_model().inventory.active_slot();
                self.session_ui.hotbar_notice.populate(
                    game.held_tool_setting().map(|label| (active, label)),
                    &mut game.notice,
                    now,
                    self.ui.state_mut(),
                );
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
            self.hud_fx.forget_health();
            self.sound.clear_world();
            // The mood eases back to the untouched image once no session
            // exists (its owner died with the world).
            renderer.set_mood([0.0, 0.0], dt);
            renderer.set_crosshair_visible(false);
            renderer.set_hand_visible(false);
            renderer.update_uniforms(
                &self.shell_camera,
                [0.60, 0.82, 1.00],
                now as f32,
                None,
                None,
            );
            let mut ui = ui_snapshot::build(None, self.screen, self.controls.pointer.cursor(), None);
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
        self.hud_fx.advance(dt);
        let hand_visible = match self.screen {
            crate::app::AppScreen::Pause | crate::app::AppScreen::Dead => false,
            crate::app::AppScreen::Sleeping => {
                self.hud_fx.sleep_hand_visible() && !game.third_person_enabled()
            }
            // Third person shows the whole body instead of the floating hand.
            _ => !game.third_person_enabled(),
        };
        renderer.set_hand_visible(hand_visible);

        // The hurt shake: a short decaying jitter on the camera look and the
        // hand's screen position. Presentation-only — the sim camera state is
        // untouched; a clone carries the offset into the uniforms.
        let shake = self.hud_fx.shake(now);
        renderer.set_hand_shake(shake.hand);

        let motion = game.local_motion(self.hud_fx.hurt_remaining());
        let listener;
        {
            if let Some(schematic) = game.tools.library.pending_save() {
                match game.schematic_scene(&schematic, 0) {
                    Ok(scene) => {
                        let jobs = game.jobs().clone();
                        let thumbnailer = renderer.schematic_thumbnailer();
                        game.tools.library
                            .start_save(&jobs, schematic, move || thumbnailer.render(&scene));
                    }
                    Err(error) => {
                        game.tools.library.abandon_save();
                        game.notice = error;
                    }
                }
            }
            let jobs = game.jobs().clone();
            let pieces = game.ghost_pieces(now, self.render_dist);
            renderer.set_anchored_ghosts(&jobs, &pieces);
            let gameplay = self.screen.gameplay_enabled();
            let preview = &game.tools.preview;
            renderer.set_schematic_preview(
                &jobs,
                (gameplay && game.schematic_preview_active())
                    .then(|| preview.scene().cloned())
                    .flatten(),
                preview.origin(),
            );
            let overlay = gameplay
                .then(|| game.tool_overlay())
                .flatten()
                .unwrap_or_default();
            renderer.set_selection_overlay(overlay.selection, overlay.corners, overlay.face);
            let frame = game.client_frame(now);
            listener = SpatialListener {
                pos: frame.camera.pos,
                right: frame.camera.right(),
            };
            let mut cam = frame.camera.clone();
            if self.options.settings.screen_shake {
                cam.yaw += shake.yaw;
                cam.pitch += shake.pitch;
            }
            renderer.set_selection(
                self.screen
                    .gameplay_enabled()
                    .then_some(frame.selection)
                    .flatten(),
            );
            let hand_events = self.hud_fx.take_hand_events();
            renderer.set_hands(
                HeldItemFrame {
                    item: frame.held_item.item,
                    display: frame.held_item.display,
                    variant: frame.held_item.variant,
                    block_state: frame.held_item.block_state,
                    mining: frame.held_item.mining,
                    eating: frame.held_item.eating,
                    pose_target: frame
                        .held_item
                        .pose_target
                        .map(crate::game::render_held_pose),
                },
                // The OFF hand: its own item + eat channel. Mining is a
                // main-hand level by definition.
                HeldItemFrame {
                    item: frame.off_hand_item.item,
                    display: frame.off_hand_item.display,
                    variant: frame.off_hand_item.variant,
                    block_state: frame.off_hand_item.block_state,
                    mining: false,
                    eating: frame.off_hand_item.eating,
                    pose_target: frame
                        .off_hand_item
                        .pose_target
                        .map(crate::game::render_held_pose),
                },
                dt,
            );
            // The first-person animator runs once both hands' frames and the
            // body's animator claims are in, and before the uniforms, which
            // wear its camera bone.
            renderer.set_local_animator(&frame.animator, &hand_events);
            renderer.set_first_person_motion(motion);
            renderer.update_uniforms(
                &cam,
                frame.environment.fog,
                frame.environment.time,
                frame.environment.eye_fluid,
                Some(&frame.environment.shader_params),
            );
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
            // Positional audio against the same snapshot the renderer draws:
            // mob sounds pin to the interpolated bodies, and the footstep and
            // idle cadences follow the live world clock, not gameplay input
            // (menus and multiplayer pause can keep that clock moving).
            self.sound.render_world(
                WorldAudioFrame {
                    listener,
                    mobs: presentation.mobs,
                    tick_alpha: presentation.tick_alpha,
                    footsteps: presentation.footsteps,
                    current_tick,
                },
                // Client-mod looping ambience (rain beds, wind): sync desired
                // gains, ease audio-side.
                |gains| game.client_mod_sound_loops(gains),
                dt,
            );
            renderer.set_mood(game.client_mod_mood(), dt);
            // The hurt vignette envelope doubles as the body's red hurt flash.
            self.scene.bake(&presentation, shake.flash);
        }
        self.scene.upload(renderer);
        let drag_preview = menu_drag_preview
            .as_ref()
            .map(|(slots, button)| (slots.as_slice(), *button));
        let mut ui =
            ui_snapshot::build(Some(game), self.screen, self.controls.pointer.cursor(), drag_preview);
        ui.craft_recipes
            .extend(self.crafting_browser.views().cloned());
        ui.craft_tip = self.crafting_browser.tip_view().cloned();
        if let Some(kind) = doc_kind {
            ui.kind = kind;
        }
        ui.hurt_flash = shake.flash;
        ui.heart_wiggle = self.hud_fx.heart_wiggle(ui.health, now);
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
