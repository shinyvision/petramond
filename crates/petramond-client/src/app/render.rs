use super::client_audio::WorldAudioFrame;
use super::hud_fx::HurtShake;
use super::screen::HandPolicy;
use super::session::Session;
use super::{ui_snapshot, App};
use crate::animation::LocalInput;
use petramond_render::{HeldItemFrame, Renderer};

impl App {
    /// Draw the current frame. The host calls this once per [`update`](Self::update);
    /// the simulation tick itself runs inside `update`, not here. Returns `false`
    /// only when a resize or screen transition made the solved UI stamp stale;
    /// the host then schedules an immediate update instead of presenting it.
    pub fn render(&mut self, renderer: &mut Renderer) -> bool {
        let now = self.now();
        // The hand animation advances by render time (not sim time); clamp so a long
        // idle gap before the first active frame can't jump a swing mid-flight.
        let dt = ((now - self.last_render) as f32).clamp(0.0, 0.1);
        self.last_render = now;
        self.apply_graphics(renderer);
        let scene_viewport = renderer.scene_ui_viewport();
        let window_viewport = renderer.window_ui_viewport();
        self.ui.set_viewport_generation(window_viewport.generation);
        self.hud_ui
            .set_viewport_generation(scene_viewport.generation);

        if self.renderer_world_clear_pending {
            renderer.clear_world_state();
            self.renderer_world_clear_pending = false;
        } else if self.renderer_moment_clear_pending {
            renderer.clear_presented_moment();
        }
        self.renderer_moment_clear_pending = false;
        // The view claims resolve ONCE, before anything below reads the
        // camera, the perspective or the chrome.
        if let Some(session) = self.session.as_mut() {
            session.game.present_view_claims();
        }
        let chrome = self
            .session
            .as_ref()
            .map(|session| session.game.view_chrome())
            .unwrap_or_default();
        let scene_screen = self.scene_screen();
        let hud_visible = self.doc_hud_active() && self.hud_claim_allows();

        // The window's document was solved in `update` (input → events →
        // controller); the HUD's is presentation-only, so it runs here.
        let doc_kind = self.doc_ui_kind();
        if let Some(kind) = doc_kind {
            if self.ui.frame_stamp() != Some((kind, window_viewport)) {
                return false;
            }
        } else {
            self.ui.deactivate();
        }
        self.solve_scene_ui(hud_visible, scene_viewport, now);
        let window_doc = self.compose_window_ui(doc_kind, window_viewport, now);
        self.compose_client_overlays(window_viewport.size);
        // The solved frame's slot cells and hooks, derived once per solve
        // into reused buffers.
        self.ui.refresh_doc_geometry();
        let menu_drag_preview = self
            .screen
            .ui_open()
            .then(|| self.ui.menu_drag_preview())
            .flatten();

        let Some(session) = self.session.as_mut() else {
            // No session: the shell sky and whatever document is up.
            // The mood eases back to the untouched image once no session
            // exists (its owner died with the world).
            renderer.set_mood([0.0, 0.0], dt);
            renderer.set_crosshair_visible(false);
            renderer.set_hand_visible(false);
            self.world_marks.clear();
            renderer.set_world_marks(&self.world_marks);
            renderer.update_uniforms(
                &self.shell_camera,
                [0.60, 0.82, 1.00],
                now as f32,
                None,
                None,
            );
            let cursor = self.controls.pointer.cursor();
            let window = ui_snapshot::build(None, self.screen, cursor, None);
            let scene = ui_snapshot::build(None, scene_screen, cursor, None);
            if !self.prepare_ui_layers(renderer, scene, window, window_doc) {
                return false;
            }
            renderer.render();
            return true;
        };
        let Session {
            game,
            sounds,
            hud_fx,
            presentation,
            scene,
            ..
        } = session;

        // A claimed camera is not the eye, so the eye's chrome (its aim marks
        // and its hand) defaults off there; a mod that aims or holds from its
        // camera asks for them back. Hiding always wins.
        let camera_claimed = game.camera_claimed();
        let eye_chrome = |claim: Option<bool>| match claim {
            Some(show) => show,
            None => !camera_claimed,
        };
        let crosshair_visible = scene_screen.gameplay_enabled() && eye_chrome(chrome.crosshair);
        renderer.set_crosshair_visible(crosshair_visible);
        hud_fx.advance(dt);
        let hand_visible = eye_chrome(chrome.hands)
            && match scene_screen.spec().hand {
                HandPolicy::Hidden => false,
                HandPolicy::SleepFade => {
                    hud_fx.sleep_hand_visible() && !game.third_person_enabled()
                }
                // Third person shows the whole body instead of the floating hand.
                HandPolicy::Shown => !game.third_person_enabled(),
            };
        // A hidden hand also drops the animator's camera kick (see
        // `update_uniforms`): the kick belongs to the viewmodel.
        renderer.set_hand_visible(hand_visible);

        // The hurt shake: a short decaying jitter on the camera look and the
        // hand's screen position. Presentation-only — the sim camera state is
        // untouched; a clone carries the offset into the uniforms.
        let own_shake = hud_fx.shake(now);
        // The captured player's eye wears the shake they felt, not ours.
        let shake = match game.captured_shake() {
            Some(captured) => HurtShake {
                yaw: captured.look[0],
                pitch: captured.look[1],
                hand: captured.hand,
                flash: captured.flash,
            },
            None => own_shake,
        };
        renderer.set_hand_shake(shake.hand);

        let motion = game.local_motion(hud_fx.hurt_remaining());
        let listener;
        let view_cue;
        {
            if let Some(schematic) = game.tools.library.pending_save() {
                match game.schematic_scene(&schematic, 0) {
                    Ok(schematic_scene) => {
                        let jobs = game.jobs().clone();
                        let thumbnailer = renderer.schematic_thumbnailer();
                        game.tools.library.start_save(&jobs, schematic, move || {
                            thumbnailer.render(&schematic_scene)
                        });
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
            // The aimed-block outline is the LOCAL look's; another player's
            // view shows their crosshair and no outline we cannot know.
            let own_aim = game.view_subject().is_none();
            let preview = &game.tools.preview;
            renderer.set_schematic_preview(
                &jobs,
                (crosshair_visible && own_aim && game.schematic_preview_active())
                    .then(|| preview.scene().cloned())
                    .flatten(),
                preview.origin(),
            );
            // The aimed-block outline, the placement preview and the held
            // tool's marks are the crosshair's kin: they draw exactly when it
            // does.
            let overlay = crosshair_visible
                .then(|| game.tool_overlay())
                .flatten()
                .unwrap_or_default();
            renderer.set_selection_overlay(overlay.selection, overlay.corners, overlay.face);
            let frame = game.client_frame(now);
            listener = frame.listener();
            let mut cam = frame.camera.clone();
            // A claimed camera is placed exactly where its mod put it.
            if self.options.settings.screen_shake && !camera_claimed {
                cam.yaw += shake.yaw;
                cam.pitch += shake.pitch;
            }
            renderer.set_selection(
                (crosshair_visible && own_aim)
                    .then_some(frame.selection)
                    .flatten(),
            );
            // The local animation opens the frame: both hands' eased poses and
            // the first-person animator (whose camera bone the uniforms wear)
            // advance once, in order, from the hands' frames, the body's
            // resolved claims and the latched rig events. Another player's eye
            // presents their hands, not ours.
            let subject = game.subject_hands();
            let input = match &subject {
                Some(subject) => LocalInput {
                    hands: subject.frames,
                    claims: &subject.animator,
                    events: &subject.events,
                    motion: subject.motion,
                    hurt_flash: shake.flash,
                },
                None => LocalInput {
                    hands: [
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
                        // The OFF hand: its own item + eat channel. Mining is
                        // a main-hand level by definition.
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
                    ],
                    claims: &frame.animator,
                    events: hud_fx.hand_events(),
                    motion,
                    // The hurt vignette envelope doubles as the body's red
                    // hurt flash.
                    hurt_flash: shake.flash,
                },
            };
            let local = presentation.animation.begin_frame(input, dt);
            renderer.set_local_frame(local);
            // The local eye as it presented, for the events logs running.
            view_cue = game.capture_wants_view().then(|| {
                let look = if self.options.settings.screen_shake {
                    [own_shake.yaw, own_shake.pitch]
                } else {
                    [0.0; 2]
                };
                game.eye_view_cue(
                    [&frame.held_item, &frame.off_hand_item],
                    &frame.animator,
                    hud_fx.hand_events(),
                    petramond::capture::view::CueShake {
                        look,
                        hand: own_shake.hand,
                        flash: own_shake.flash,
                    },
                    &motion,
                )
            });
            hud_fx.clear_hand_events();
            renderer.update_uniforms(
                &cam,
                frame.environment.fog,
                frame.environment.time,
                frame.environment.eye_fluid,
                Some(&frame.environment.shader_params),
            );
            // What this frame PRESENTS, recorded once where it is decided, for
            // the client-mod hook to read back as `ClientViewState`.
            self.presented_view = mod_api::ClientViewStateData {
                third_person: game.third_person_enabled(),
                hud_visible,
                hands_visible: hand_visible,
                crosshair_visible,
                camera_claimed,
                fov_y: cam.fov_y,
                pos: cam.pos.to_array(),
                yaw: cam.yaw,
                pitch: cam.pitch,
                roll: cam.roll,
                frame_size: {
                    let (w, h) = renderer.screen_size();
                    [w, h]
                },
                subject: game.view_subject(),
                anchor_missing: game.camera_anchor_missing(),
                world_settled: false,
            };
            // After the uniforms: marks bake against the origin they publish.
            super::world_marks::compose(&mut self.world_marks, game, window_viewport.scale);
            renderer.set_world_marks(&self.world_marks);
        }
        game.capture_frame_end(view_cue);
        // Build the neutral read snapshot, then bake it into render wire structs.
        {
            let current_tick = game.current_tick();
            // The same hourly-wrapped clock `GameEnvironment::time` carries, so
            // ambient volumes animate on the exact clock looping emitters do.
            // The renderer's own view volume, published by `update_uniforms`
            // above (shake included) and unchanged until this frame draws — so
            // gathers cull against exactly what will be rasterized.
            let view = renderer.view_volume();
            let snapshot =
                presentation.snapshot(game, (game.world_clock(now) % 3600.0) as f32, &view);
            renderer.set_break_overlays(snapshot.break_overlays);
            // Positional audio against the same snapshot the renderer draws:
            // mob sounds pin to the interpolated bodies, and the footstep and
            // idle cadences follow the live world clock, not gameplay input
            // (menus and multiplayer pause can keep that clock moving).
            self.sound.render_world(
                sounds,
                WorldAudioFrame {
                    listener,
                    mobs: snapshot.mobs,
                    tick_alpha: snapshot.tick_alpha,
                    footsteps: snapshot.footsteps,
                    current_tick,
                },
                // Client-mod looping ambience (rain beds, wind): sync desired
                // gains, ease audio-side.
                |gains| game.client_mod_sound_loops(gains),
                dt,
            );
            renderer.set_mood(game.client_mod_mood(), dt);
            scene.bake(&snapshot);
        }
        scene.upload(renderer);
        let drag_preview = menu_drag_preview
            .as_ref()
            .map(|(slots, button)| (slots.as_slice(), *button));
        let cursor = self.controls.pointer.cursor();
        let mut window_ui = ui_snapshot::build(Some(&*game), self.screen, cursor, drag_preview);
        window_ui
            .craft_recipes
            .extend(self.crafting_browser.views().cloned());
        window_ui.craft_tip = self.crafting_browser.tip_view().cloned();
        let mut scene_ui = ui_snapshot::build(Some(&*game), scene_screen, cursor, None);
        // The hurt vignette is HUD; the body's own red flash above is not.
        scene_ui.hurt_flash = if hud_visible { shake.flash } else { 0.0 };
        scene_ui.heart_wiggle = hud_fx.heart_wiggle(scene_ui.health, now);
        if !self.prepare_ui_layers(renderer, scene_ui, window_ui, window_doc) {
            return false;
        }

        let game = &mut self
            .session
            .as_mut()
            .expect("the session drawn above still exists")
            .game;
        let terrain_busy = {
            let mut terrain = game.terrain_render_handoff();
            renderer.sync_meshes(&mut terrain);
            terrain.is_streaming()
        };
        // The one readiness rule: a `Settled` capture is taken by it.
        self.presented_view.world_settled = !terrain_busy && !renderer.terrain_uploads_pending();
        self.present_frame(renderer);
        self.heap_reclaim.frame(
            terrain_busy || renderer.terrain_uploads_pending(),
            std::time::Instant::now(),
        );
        true
    }

    /// Whether the view claims leave the HUD up.
    pub(super) fn hud_claim_allows(&self) -> bool {
        self.session
            .as_ref()
            .is_none_or(|session| session.game.view_chrome().hud != Some(false))
    }
}
