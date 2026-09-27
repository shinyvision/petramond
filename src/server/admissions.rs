//! Join admission off the server thread.
//!
//! Admitting an authenticated player costs a player-file read (plus, for a
//! pre-identity world, a legacy-file migration), a registry write, and —
//! for a first-time player — a spawn search that runs worldgen. None of
//! that may stall the tick every connected player shares, so a join runs in
//! two halves:
//!
//! 1. [`ServerGame::begin_admission`] (server thread, no I/O): the admission
//!    check, a `PlayerId` and the display name are RESERVED at once, so
//!    concurrent joins resolve against each other exactly as against
//!    connected sessions; the restore then goes to the job pool.
//! 2. [`ServerGame::take_admitted`] / [`ServerGame::finish_admission`]
//!    (server thread, at the next pump boundary): the restored player
//!    becomes a session — or, if its connection could not be promoted,
//!    [`ServerGame::abandon_admission`] releases the reservation.
//!
//! A restore job that panics still reports back (as a failed admission), so
//! a reservation can never leak and its connection never waits forever.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use crate::net::identity::PlayerKey;
use crate::net::protocol::{
    ItemSlotWire, JoinData, JoinRejectReason, SectionCacheClaim, SelfRestore,
};
use crate::player::{Player, PlayerId};
use crate::server::accounts;
use crate::server::game::ServerGame;
use crate::server::player::ConnectedPlayer;
use crate::worker::JobPool;

const RESTORE_PRIORITY: i64 = i64::MIN + 2;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct AdmissionTicket(u64);

struct InFlight {
    ticket: AdmissionTicket,
    key: PlayerKey,
    id: PlayerId,
    name: String,
    view_distance: i32,
    cached_sections: Vec<SectionCacheClaim>,
}

pub struct Admitted {
    ticket: AdmissionTicket,
    restored: Option<(Player, bool)>,
}

impl Admitted {
    pub fn ticket(&self) -> AdmissionTicket {
        self.ticket
    }
}

pub struct Admissions {
    jobs: Arc<JobPool>,
    in_flight: Vec<InFlight>,
    done_tx: Sender<Admitted>,
    done_rx: Receiver<Admitted>,
    next_ticket: u64,
}

impl Admissions {
    pub fn new(jobs: Arc<JobPool>) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        Self {
            jobs,
            in_flight: Vec::new(),
            done_tx,
            done_rx,
            next_ticket: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_empty(&self) -> bool {
        self.in_flight.is_empty()
    }

    fn holds_key(&self, key: &PlayerKey) -> bool {
        self.in_flight.iter().any(|f| f.key == *key)
    }

    fn holds_id(&self, id: PlayerId) -> bool {
        self.in_flight.iter().any(|f| f.id == id)
    }

    fn holds_name(&self, key: &PlayerKey, candidate: &str) -> bool {
        self.in_flight
            .iter()
            .any(|f| f.key != *key && f.name.eq_ignore_ascii_case(candidate))
    }

    fn take(&mut self, ticket: AdmissionTicket) -> Option<InFlight> {
        let at = self.in_flight.iter().position(|f| f.ticket == ticket)?;
        Some(self.in_flight.swap_remove(at))
    }

    fn drain(&mut self) -> Vec<Admitted> {
        self.done_rx.try_iter().collect()
    }
}

struct ReportGuard {
    ticket: AdmissionTicket,
    tx: Option<Sender<Admitted>>,
}

impl ReportGuard {
    fn report(mut self, player: Player, first_seen: bool) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Admitted {
                ticket: self.ticket,
                restored: Some((player, first_seen)),
            });
        }
    }
}

impl Drop for ReportGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            log::error!("restoring a joining player failed; the join is dropped");
            let _ = tx.send(Admitted {
                ticket: self.ticket,
                restored: None,
            });
        }
    }
}

impl ServerGame {
    pub fn check_admission(&self, key: &PlayerKey) -> Result<(), JoinRejectReason> {
        if self.sessions.iter().any(|s| s.key == *key) || self.admissions.holds_key(key) {
            return Err(JoinRejectReason::AlreadyConnected);
        }
        self.free_player_id().map(drop)
    }

    fn free_player_id(&self) -> Result<PlayerId, JoinRejectReason> {
        if self.sessions.len() + self.admissions.len() >= self.sessions.capacity() {
            return Err(JoinRejectReason::ServerFull);
        }
        (0..=u8::MAX)
            .map(PlayerId)
            .find(|id| self.sessions.by_id(*id).is_none() && !self.admissions.holds_id(*id))
            .ok_or(JoinRejectReason::ServerFull)
    }

    pub fn begin_admission(
        &mut self,
        key: PlayerKey,
        requested: &str,
        view_distance: i32,
        cached_sections: Vec<SectionCacheClaim>,
    ) -> Result<AdmissionTicket, JoinRejectReason> {
        self.check_admission(&key)?;
        let id = self.free_player_id()?;
        let (sessions, admissions) = (&self.sessions, &self.admissions);
        let reservation = self.accounts.reserve(key, requested, |candidate| {
            admissions.holds_name(&key, candidate)
                || sessions
                    .iter()
                    .any(|s| s.key != key && s.name.eq_ignore_ascii_case(candidate))
        });
        let ticket = AdmissionTicket(self.admissions.next_ticket);
        self.admissions.next_ticket += 1;
        self.admissions.in_flight.push(InFlight {
            ticket,
            key,
            id,
            name: reservation.name.clone(),
            view_distance,
            cached_sections,
        });
        let files = self.world.save().map(crate::save::WorldSave::player_files);
        let seed = self.world.data().seed;
        let guard = ReportGuard {
            ticket,
            tx: Some(self.admissions.done_tx.clone()),
        };
        let (name, known, changed) = (reservation.name, reservation.known, reservation.changed);
        self.admissions.jobs.submit(RESTORE_PRIORITY, move || {
            let restored = accounts::restore(files.as_deref(), key, &name, known);
            if let (Some(files), Some(changed)) = (&files, &changed) {
                changed.write(files);
            }
            let player = restored
                .player
                .unwrap_or_else(|| crate::server::session_build::spawn_player(seed));
            guard.report(player, restored.first_seen);
        });
        Ok(ticket)
    }

    pub fn take_admitted(&mut self) -> Vec<Admitted> {
        self.admissions.drain()
    }

    pub fn finish_admission(&mut self, admitted: Admitted) -> Option<(Box<JoinData>, String)> {
        let flight = self.admissions.take(admitted.ticket)?;
        let (mut player, first_seen) = admitted.restored?;
        if first_seen && self.operators.claim_legacy(&flight.name, flight.key) {
            crate::server::permissions::store(&mut self.world, &self.operators);
        }
        crate::server::progression::catch_up(&mut player, self.catalog.unlocks());
        let data = Box::new(JoinData {
            player_id: flight.id,
            player_name: flight.name.clone(),
            seed: self.world.data().seed,
            clock: crate::server::daynight::current_clock(&self.world),
            tables: crate::net::remap::local_name_tables(),
            self_restore: self_restore_from(&player),
            crafting_recipes: self.catalog.recipes().crafting().to_data(),
            players: self
                .sessions
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect(),
            client_policy: self.client_policy,
        });
        let mut session = ConnectedPlayer::new(
            flight.id,
            flight.key,
            flight.name.clone(),
            player,
            flight.view_distance,
        );
        session.replication.sent_unlock_count = session.player.progression.unlocked().len();
        session
            .transport
            .terrain
            .seed_client_cache(&flight.cached_sections);
        self.broadcast.reseed_env();
        self.sessions.join(session);
        Some((data, flight.name))
    }

    pub fn abandon_admission(&mut self, admitted: Admitted) {
        self.admissions.take(admitted.ticket);
    }

    #[cfg(test)]
    pub fn admit_remote_player(
        &mut self,
        key: PlayerKey,
        requested: &str,
        view_distance: i32,
        cached_sections: &[SectionCacheClaim],
    ) -> Result<(Box<JoinData>, String), JoinRejectReason> {
        let ticket =
            self.begin_admission(key, requested, view_distance, cached_sections.to_vec())?;
        loop {
            let admitted = self
                .admissions
                .done_rx
                .recv()
                .expect("the admissions lane outlives its jobs");
            if admitted.ticket == ticket {
                return self
                    .finish_admission(admitted)
                    .ok_or(JoinRejectReason::ServerFull);
            }
            self.abandon_admission(admitted);
        }
    }
}

pub(in crate::server) fn self_restore_from(player: &Player) -> SelfRestore {
    SelfRestore {
        transform: crate::net::protocol::Transform {
            pos: player.pos,
            vel: player.vel,
            yaw: player.yaw,
            pitch: player.pitch,
        },
        mode: player.mode().to_u8(),
        health: player.health(),
        bed_spawn: player.bed_spawn.map(|b| (b.bed, b.spot)),
        effects: player
            .effects()
            .iter()
            .map(|e| (e.effect.def().name.to_string(), e.remaining))
            .collect(),
        inventory: player
            .inventory
            .raw_slots()
            .iter()
            .copied()
            .chain(std::iter::once(player.inventory.cursor().copied()))
            .chain(std::iter::once(player.inventory.off_hand().copied()))
            .map(|slot| slot.map(ItemSlotWire::from_stack))
            .collect(),
        active_slot: player.inventory.active_slot(),
        craft_craftable_only: player.craft_craftable_only,
        unlocked_recipes: player.progression.unlocked().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> PlayerKey {
        PlayerKey([byte; 32])
    }

    #[test]
    fn in_flight_joins_reserve_their_identity_id_and_name() {
        let mut server = crate::server::session_build::build_server_inline("", 5, 2);
        server.set_max_players(3);
        let local = server.sessions[0].name.clone();
        let requested = if local.eq_ignore_ascii_case("Rachel") {
            "Visitor"
        } else {
            "Rachel"
        };

        let first = server
            .begin_admission(key(1), requested, 8, Vec::new())
            .expect("room for one");
        assert_eq!(
            server.check_admission(&key(1)),
            Err(JoinRejectReason::AlreadyConnected),
            "a joining identity cannot join twice"
        );
        let second = server
            .begin_admission(key(2), &requested.to_ascii_lowercase(), 8, Vec::new())
            .expect("room for two");
        assert_eq!(
            server.begin_admission(key(3), "Third", 8, Vec::new()),
            Err(JoinRejectReason::ServerFull),
            "joins in flight count against the cap"
        );

        let mut admitted = server.take_admitted();
        admitted.sort_by_key(|a| a.ticket().0);
        assert_eq!(
            admitted.iter().map(Admitted::ticket).collect::<Vec<_>>(),
            [first, second],
            "the inline pool restored both at once"
        );
        let mut admitted = admitted.into_iter();
        let (data, name) = server
            .finish_admission(admitted.next().expect("first"))
            .expect("restored");
        assert_eq!((data.player_id, name.as_str()), (PlayerId(1), requested));
        assert!(data.players.iter().any(|(_, n)| *n == local));
        server.abandon_admission(admitted.next().expect("second"));
        assert_eq!(
            server.sessions.len(),
            2,
            "an abandoned join adds no session"
        );
        assert!(
            server.check_admission(&key(2)).is_ok(),
            "an abandoned join releases its reservation"
        );
    }

    #[test]
    fn a_failed_restore_releases_its_reservation() {
        let mut server = crate::server::session_build::build_server_inline("", 5, 2);
        let ticket = AdmissionTicket(99);
        server.admissions.in_flight.push(InFlight {
            ticket,
            key: key(7),
            id: PlayerId(9),
            name: "Ghost".into(),
            view_distance: 8,
            cached_sections: Vec::new(),
        });
        drop(ReportGuard {
            ticket,
            tx: Some(server.admissions.done_tx.clone()),
        });
        let admitted = server.take_admitted();
        assert_eq!(admitted.len(), 1);
        let admitted = admitted.into_iter().next().expect("reported");
        assert!(server.finish_admission(admitted).is_none());
        assert!(server.admissions.is_empty());
        assert!(server.check_admission(&key(7)).is_ok());
    }
}
