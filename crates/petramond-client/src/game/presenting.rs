//! A client presenting a world from a mod's files: world-less (no server, no
//! save, no simulation), opened from the shell by the pack launched there,
//! which goes on driving it through the presentation desk.
//!
//! Each frame, before anything reads the world, the desk's calls take effect
//! in issue order and the presentation drives: what it lands or releases
//! arrives as the same world messages a live client ingests (their terrain
//! already on the replica), through the one pump. The position is the one
//! owner of presented time: the interpolation pair, the world's frame time
//! and the shader clock all follow it.

use std::sync::Arc;

use mod_api::ClientPose;
use petramond::capture::body::NameTables;
use petramond::capture::present::{DriveOut, Presentation};
use petramond::capture::window::Window;
use petramond::modding::client::present::{Published, Request};
use petramond::modding::client::ClientModRuntime;
use petramond::net::handle::ServerHandle;
use petramond::net::protocol::{SelfEvents, ServerToClient, TickSection};
use petramond::player::{Player, PlayerId, PlayerMode};
use petramond::worker::JobPool;
use petramond::world::ReplicaWorld;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_world::block::Block;
use petramond_world::crafting::CraftingCatalog;
use petramond_worldgen::density::surface::SurfaceDensitySystem;

use super::session::ClientBootstrap;
use super::Game;

/// What this client presents besides a live session's world.
#[derive(Default)]
pub(super) struct Presenting {
    /// The presentation, when this client is one.
    engine: Option<Box<Presentation>>,
    /// The captured player's first person, presented when they are the view
    /// subject.
    pub(super) view: super::captured_view::ViewCues,
    /// The captured player's dig cell.
    dig: Option<IVec3>,
    /// The position the world's frame time was last taken from.
    last_position: f64,
    /// The world's frame time this frame: how far the position moved.
    world_step: f32,
    /// The owner asked to close, or stopped running: the app ends the client.
    closed: bool,
    /// The owner opened another presentation over this one.
    reopen: Option<Request>,
    /// The owner placed the viewer: until it does (or a camera claim puts
    /// one up) there is no window, and nothing is resident.
    viewer_placed: bool,
    replaced: bool,
    time_jumped: bool,
}

impl Game {
    /// A client presenting a world with `seed`, whose ids are `tables`: an
    /// empty replica, a viewer body at `viewer` (PlayerId `u8::MAX`, which no
    /// capture uses), and `runtime` — the owner carried over beside the
    /// presented world's own client mods.
    pub fn new_presentation(
        cam: Camera,
        render_dist: i32,
        seed: u32,
        tables: NameTables,
        viewer: Option<ClientPose>,
        runtime: ClientModRuntime,
    ) -> Self {
        let jobs = Arc::new(JobPool::new(JobPool::default_threads()));
        let mut replica = ReplicaWorld::with_pool(seed, render_dist, Arc::clone(&jobs));
        replica.keep_provenance();
        let pose = viewer.unwrap_or(ClientPose {
            pos: [0.0, 80.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        });
        let eye = WorldPos::new(pose.pos[0], pose.pos[1], pose.pos[2]);
        let mut player = Player::new(eye - Vec3::new(0.0, petramond::player::EYE, 0.0));
        player.set_mode(PlayerMode::Spectator);
        player.yaw = pose.yaw;
        player.pitch = pose.pitch;
        let bootstrap = ClientBootstrap::presented(
            replica,
            Arc::clone(&jobs),
            player,
            PlayerId(u8::MAX),
            SurfaceDensitySystem::new(seed),
            runtime,
            CraftingCatalog::default(),
        );
        let mut game = Self::assemble(cam, ServerHandle::serverless(), bootstrap);
        game.presenting.engine = Some(Box::new(Presentation::new(tables, jobs)));
        game.presenting.replaced = true;
        game.presenting.viewer_placed = viewer.is_some();
        game.client_mods.drop_view_claims();
        game.publish_presentation(&[], &[], None);
        game
    }

    /// Whether this client presents a world from mod files.
    pub fn in_presentation(&self) -> bool {
        self.presenting.engine.is_some()
    }

    /// The presentation was closed, or its owner stopped (taken once): the
    /// app ends this client and gives the owner back to the shell.
    pub fn take_presentation_closed(&mut self) -> bool {
        std::mem::take(&mut self.presenting.closed)
    }

    /// The owner opened another presentation over this one (taken once).
    pub fn take_presentation_reopen(&mut self) -> Option<Request> {
        self.presenting.reopen.take()
    }

    /// End a presentation: its owner back on the shell, or `None` when it
    /// did not survive (it trapped). The client is spent.
    pub fn into_shell(mut self) -> Option<ClientModRuntime> {
        debug_assert!(self.in_presentation(), "only a presentation has a shell");
        std::mem::replace(&mut self.client_mods, ClientModRuntime::empty()).into_shell()
    }

    /// End a presentation for good, however it was left (its owner closed
    /// it, or the player left from the pause menu): the owner hears it
    /// ended, exactly as if it had closed it itself.
    pub fn leave_presentation(self) -> Option<ClientModRuntime> {
        {
            let mut desk = self.client_mods.presented().lock();
            if desk.presentation.owner().is_some() {
                desk.presentation.ended(None);
            }
        }
        self.into_shell()
    }

    /// The presented position, fractional ticks.
    pub(crate) fn presentation_position(&self) -> Option<f64> {
        self.presenting.engine.as_ref().map(|e| e.position())
    }

    /// The captured player, whose view the `View` pieces carry.
    pub(super) fn captured_player(&self) -> Option<mod_api::PlayerId> {
        let engine = self.presenting.engine.as_ref()?;
        engine.moment().local_player.map(|p| mod_api::PlayerId(p.0))
    }

    /// The clock world animation runs on (shaders, ambient volumes): a
    /// presentation's position, else the app's.
    pub fn world_clock(&self, now: f64) -> f64 {
        match self.presentation_position() {
            Some(at) => at * f64::from(petramond::events::tick::TICK_DT),
            None => now,
        }
    }

    /// Frame time as the presented world experiences it: exactly as far as
    /// the position moved (none while it holds still).
    pub(super) fn world_dt(&self, dt: f32) -> f32 {
        if self.in_presentation() {
            self.presenting.world_step
        } else {
            dt
        }
    }

    /// The block the dig loop sounds: the captured player's dig in a
    /// presentation, this player's own while the break button holds one.
    pub fn dig_loop_block(&self, break_held: bool) -> Option<Block> {
        let cell = if self.in_presentation() {
            self.presenting.dig
        } else {
            self.replica
                .self_view
                .mining
                .filter(|_| break_held)
                .map(|(cell, _)| cell)
        }?;
        Some(Block::from_id(
            self.replica
                .world
                .data()
                .chunk_block(cell.x, cell.y, cell.z),
        ))
    }

    /// Carry out the presentation desk's calls, in issue order, and drive
    /// the presentation one frame. Runs first in the frame: the position it
    /// leaves is what every later read of presented time sees.
    pub(super) fn drive_presentation(&mut self, dt: f32) {
        if self.presenting.engine.is_none() {
            return;
        }
        let desk = self.client_mods.presented().clone();
        let requests = desk.lock().presentation.take_requests();
        let owner = desk.lock().presentation.owner().map(str::to_owned);
        let owner_alive = owner
            .as_deref()
            .is_some_and(|o| self.client_mods.is_live(o));
        let mut failed = Vec::new();
        for request in requests {
            let engine = self.presenting.engine.as_mut().expect("checked above");
            match request {
                Request::Op(op) => engine.push(op),
                Request::Cancel(id) => {
                    engine.cancel(id);
                }
                Request::Viewer { pose, flying } => self.place_viewer(pose, flying),
                Request::Close => self.presenting.closed = true,
                open @ Request::Open { .. } => self.presenting.reopen = Some(open),
            }
        }
        if !owner_alive && !self.presenting.closed {
            log::warn!("presentation closed: its owner stopped running");
            desk.lock()
                .presentation
                .ended(Some("the presentation's owner stopped running".into()));
            self.presenting.closed = true;
        }
        let eye = self.render_camera().pos;
        let window = (self.presenting.viewer_placed || self.camera_claimed()).then(|| Window {
            center: petramond_world::chunk::ChunkPos::new(
                (eye.x.floor() as i32).div_euclid(16),
                (eye.z.floor() as i32).div_euclid(16),
            ),
            radius: self.replica.world.data().render_dist,
        });
        let engine = self.presenting.engine.as_mut().expect("checked above");
        engine.set_window(window);
        let out = engine.drive(&mut self.replica.world, dt);
        let position = engine.position();
        self.presenting.world_step = if out.jumped {
            0.0
        } else {
            ((position - self.presenting.last_position).max(0.0)
                * f64::from(petramond::events::tick::TICK_DT)) as f32
        };
        self.presenting.last_position = position;
        let landed = out.landed.clone();
        failed.extend(out.failed.iter().map(|(id, _)| *id));
        self.present_drive(out);
        self.publish_presentation(&landed, &failed, None);
    }

    /// Hand what the drive released to the ingest and the presenters.
    fn present_drive(&mut self, out: DriveOut) {
        if out.jumped {
            self.reset_presented_moment();
            // An events log is one continuous stretch of one world.
            let capture = self.client_mods.presented().lock().capture.clone();
            let mut desk = capture.lock();
            for owner in desk.logs.owners() {
                desk.logs
                    .end_all_of(&owner, "an apply moved the presented world");
            }
        }
        for mut msg in out.messages {
            if let ServerToClient::Tick(t) = &mut msg {
                // The captured player's own state is their HUD's, never the
                // viewer's; their one-shots are theirs too, but for the hurt
                // bark, a sound the world made.
                let own = &mut self.presenting.view.own;
                t.sections.retain_mut(|section| match section {
                    TickSection::SelfState(state) => {
                        own.get_or_insert_with(super::replicated::SelfView::nobody)
                            .apply(state, true);
                        false
                    }
                    TickSection::SelfEvents(events) => {
                        *events = SelfEvents {
                            player_damaged: events.player_damaged,
                            ..Default::default()
                        };
                        true
                    }
                    _ => true,
                });
            }
            self.net.inject(msg);
        }
        let suppress = rustc_hash::FxHashSet::default();
        for cues in out.cues {
            self.presenting.dig = cues.dig;
            for event in cues.events {
                self.buffer_world_event(event, &suppress);
            }
        }
        for view in out.views {
            self.receive_view_cue(view);
        }
    }

    /// Everything that presents a MOMENT of the world starts over: the
    /// landed apply's moment arrives in full right after, over an empty
    /// entity store (its lanes spawn what it holds).
    fn reset_presented_moment(&mut self) {
        let self_id = self.replica.entities.self_id();
        self.replica.entities = super::replicated::EntityReplica::new(self_id, []);
        self.capture_entities_reset();
        self.fx.clear_moment();
        self.replica.events = Default::default();
        self.presented_entities_cache.clear();
        self.last_anchor_feet = None;
        self.presenting.dig = None;
        self.presenting.view = Default::default();
        self.presenting.time_jumped = true;
    }

    /// The owner places the viewer's EYE at `pose`; `flying` flies it free,
    /// else it walks the resident world with gravity and collision.
    fn place_viewer(&mut self, pose: ClientPose, flying: bool) {
        let eye = WorldPos::new(pose.pos[0], pose.pos[1], pose.pos[2]);
        let feet = eye - Vec3::new(0.0, petramond::player::EYE, 0.0);
        let player = &mut self.local.player;
        player.set_mode(if flying {
            PlayerMode::Spectator
        } else {
            PlayerMode::Survival
        });
        player.teleport(feet);
        player.yaw = pose.yaw;
        player.pitch = pose.pitch;
        let cam = &mut self.local.cam;
        cam.pos = eye;
        cam.yaw = pose.yaw;
        cam.pitch = pose.pitch;
        self.presenting.viewer_placed = true;
    }

    /// Tell the desk where the presentation stands.
    fn publish_presentation(&mut self, landed: &[u64], failed: &[u64], error: Option<String>) {
        let Some(engine) = self.presenting.engine.as_ref() else {
            return;
        };
        let state = Published {
            opening: false,
            open: true,
            position: engine.position(),
            released_through: engine.released_through(),
            ready: engine.ready(),
            applied: engine.applied(),
            exhausted: engine.exhausted(),
            error: error.or_else(|| engine.error().map(str::to_owned)),
        };
        self.client_mods
            .presented()
            .lock()
            .presentation
            .publish(state, landed, failed);
    }

    /// The world this client presents was swapped since the last frame's
    /// events (taken once).
    pub(super) fn take_world_replaced(&mut self) -> bool {
        std::mem::take(&mut self.presenting.replaced)
    }

    /// The presented world jumped (an apply landed) since the last frame's
    /// events (taken once).
    pub(super) fn take_time_jumped(&mut self) -> bool {
        std::mem::take(&mut self.presenting.time_jumped)
    }
}
