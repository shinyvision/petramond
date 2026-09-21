//! The client half of schematic sharing: a choice the server opened (the
//! library screen), uploading a chosen archive the world lacks, positioning a
//! world-held design with the placement controls, fetching designs this
//! client does not hold, and the anchored ghosts it should draw.
//!
//! The personal library is only read here; downloaded designs live in memory.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond::schematic::share::{
    BlobPacket, BlobReceiver, BlobSender, GhostPlacement, SchematicNotice, SchematicRequest,
};
use petramond::schematic::store::{digest, Digest};
use petramond::schematic::{archive, Schematic};
use petramond_render::job::Job;

use super::Game;

/// How many decoded designs this client keeps.
const DESIGNS_RESIDENT: usize = 16;

type Prepared = Result<(Digest, Arc<[u8]>, Arc<Schematic>), String>;
type Encoded = Result<(Digest, Arc<[u8]>), String>;

#[derive(Default)]
pub struct SchematicShare {
    /// The open choice the server asked for.
    pub choice: Option<String>,
    /// A choice arrived that the app has not opened a screen for yet.
    open_library: bool,
    /// The library entry being read and hashed for the open choice.
    choosing: Option<(String, Job<Prepared>)>,
    /// A paste being encoded for the wire, and where it goes.
    pasting: Option<(Job<Encoded>, [i32; 3], u8)>,
    /// Captures the server said are on their way, saved once they decode.
    captures: std::collections::HashSet<Digest>,
    /// The chosen archive, kept until the server says whether it needs it.
    offered: Option<(Digest, Arc<[u8]>)>,
    upload: Option<BlobSender>,
    downloads: HashMap<Digest, BlobReceiver>,
    decoding: HashMap<Digest, Job<Result<Schematic, String>>>,
    designs: HashMap<Digest, Arc<Schematic>>,
    design_order: Vec<Digest>,
    /// A positioning the server opened, waiting for its design.
    positioning: Option<(String, Digest, Option<[i32; 3]>, u8)>,
    /// Anchored ghosts by key, as the server last sent them.
    pub ghosts: BTreeMap<String, GhostPlacement>,
}

impl Game {
    pub(super) fn receive_schematic_notices(&mut self, notices: Vec<SchematicNotice>) {
        for notice in notices {
            match notice {
                SchematicNotice::Choose { tag } => {
                    self.schematics.choice = Some(tag);
                    self.schematics.open_library = true;
                }
                SchematicNotice::Position {
                    tag,
                    digest,
                    origin,
                    turns,
                } => {
                    self.schematics.positioning = Some((tag, digest, origin, turns));
                    self.want_design(digest);
                }
                SchematicNotice::Want { digest } => {
                    if let Some((offered, bytes)) = self.schematics.offered.take() {
                        if offered == digest {
                            self.schematics.upload = Some(BlobSender::new(digest, bytes));
                        }
                    }
                }
                SchematicNotice::Ghost { key, placement } => match placement {
                    Some(placement) => {
                        self.want_design(placement.digest);
                        self.schematics.ghosts.insert(key, placement);
                    }
                    None => {
                        self.schematics.ghosts.remove(&key);
                    }
                },
                SchematicNotice::Refused { message } => self.notice = message,
                SchematicNotice::Blob(packet) => self.receive_schematic_blob(packet),
            }
        }
    }

    fn receive_schematic_blob(&mut self, packet: BlobPacket) {
        let share = &mut self.schematics;
        match packet {
            BlobPacket::Begin { digest, .. } => {
                if share.downloads.contains_key(&digest) {
                    return;
                }
                match BlobReceiver::begin(&packet, digest, archive::MAX_ARCHIVE_BYTES) {
                    Ok(receiver) => {
                        share.downloads.insert(digest, receiver);
                    }
                    Err(message) => self.notice = message,
                }
            }
            BlobPacket::Data { digest, .. } => {
                if let Some(download) = share.downloads.get_mut(&digest) {
                    if let Err(message) = download.receive(packet) {
                        share.downloads.remove(&digest);
                        self.notice = message;
                    }
                }
            }
            BlobPacket::Credit { digest, count } => {
                if let Some(upload) = share.upload.as_mut().filter(|u| u.digest() == digest) {
                    if !upload.credit(count) {
                        share.upload = None;
                    }
                }
            }
            BlobPacket::Cancel { digest } => {
                share.downloads.remove(&digest);
                if share.upload.as_ref().is_some_and(|u| u.digest() == digest) {
                    share.upload = None;
                }
            }
        }
    }

    /// Make `digest` available: decoded already, or fetched from the server.
    fn want_design(&mut self, digest: Digest) {
        let share = &self.schematics;
        if share.designs.contains_key(&digest)
            || share.decoding.contains_key(&digest)
            || share.downloads.contains_key(&digest)
        {
            return;
        }
        self.outbox
            .push(ClientToServer::Action(PlayerAction::Schematic(
                SchematicRequest::Fetch { digest },
            )));
    }

    /// Paste `schematic` at `origin`: its archive is offered to the server
    /// under its digest, which asks for the bytes only if the world lacks them.
    pub(super) fn paste_schematic(
        &mut self,
        schematic: Arc<Schematic>,
        origin: [i32; 3],
        turns: u8,
    ) {
        if self.schematics.pasting.is_some() || self.schematics.upload.is_some() {
            self.notice = "A schematic is still being transferred".into();
            return;
        }
        let job = Job::spawn(&self.jobs, move || {
            let bytes: Arc<[u8]> = archive::encode_bare(&schematic)?.into();
            Ok((digest(&bytes), bytes))
        });
        self.schematics.pasting = Some((job, origin, turns));
    }

    /// The server captured a selection: its blob `digest` is saved to the
    /// library once it is here.
    pub(super) fn expect_capture(&mut self, digest: Digest) {
        self.schematics.captures.insert(digest);
    }

    /// A decoded design this client holds.
    pub fn schematic_design(&self, digest: &Digest) -> Option<&Arc<Schematic>> {
        self.schematics.designs.get(digest)
    }

    fn keep_design(&mut self, digest: Digest, schematic: Arc<Schematic>) {
        let share = &mut self.schematics;
        share.designs.insert(digest, schematic);
        share.design_order.retain(|d| *d != digest);
        share.design_order.push(digest);
        while share.design_order.len() > DESIGNS_RESIDENT {
            let old = share.design_order.remove(0);
            let referenced = share.ghosts.values().any(|g| g.digest == old)
                || share.positioning.as_ref().is_some_and(|p| p.1 == old);
            if referenced {
                share.design_order.push(old);
                break;
            }
            share.designs.remove(&old);
        }
    }

    /// Whether the app should open the library for a new choice (an edge).
    pub fn take_schematic_library_request(&mut self) -> bool {
        std::mem::take(&mut self.schematics.open_library)
    }

    /// The library screen is up for a choice, not for creative placement.
    pub fn schematic_choice_open(&self) -> bool {
        self.schematics.choice.is_some()
    }

    /// Choose library entry `index` for the open choice: read and hash it
    /// off the frame thread, then tell the server.
    pub fn choose_schematic(&mut self, index: usize) -> bool {
        let Some(tag) = self.schematics.choice.clone() else {
            return false;
        };
        let Some(entry) = self.schematic_library.entries().get(index) else {
            return false;
        };
        let path = entry.path.clone();
        self.schematics.choosing = Some((
            tag,
            Job::spawn(&self.jobs, move || {
                let bytes: Arc<[u8]> = std::fs::read(&path).map_err(|e| e.to_string())?.into();
                let schematic = archive::decode(&bytes)?;
                Ok((digest(&bytes), bytes, Arc::new(schematic)))
            }),
        ));
        true
    }

    /// Advance the sharing lane once per frame.
    pub(super) fn poll_schematic_share(&mut self) {
        let chosen = self
            .schematics
            .choosing
            .as_ref()
            .and_then(|(_, job)| job.poll());
        if let Some(outcome) = chosen {
            let (tag, _) = self.schematics.choosing.take().expect("polled above");
            match outcome.unwrap_or_else(|_| Err("Reading the schematic failed".into())) {
                Ok((id, bytes, schematic)) => {
                    self.keep_design(id, schematic);
                    self.schematics.offered = Some((id, bytes));
                    self.schematics.choice = None;
                    self.outbox
                        .push(ClientToServer::Action(PlayerAction::Schematic(
                            SchematicRequest::Chosen { tag, digest: id },
                        )));
                }
                Err(message) => self.notice = message,
            }
        }
        let encoded = self
            .schematics
            .pasting
            .as_ref()
            .and_then(|(job, ..)| job.poll());
        if let Some(outcome) = encoded {
            let (_, origin, turns) = self.schematics.pasting.take().expect("polled above");
            match outcome.unwrap_or_else(|_| Err("Encoding the schematic failed".into())) {
                Ok((id, bytes)) => {
                    self.schematics.offered = Some((id, bytes));
                    self.outbox
                        .push(ClientToServer::Action(PlayerAction::Creative(
                            petramond::schematic::CreativeAction::Place {
                                digest: id,
                                origin,
                                turns,
                            },
                        )));
                }
                Err(message) => self.notice = message,
            }
        }
        if let Some(upload) = self.schematics.upload.as_mut() {
            let packets = upload.packets();
            let finished = upload.finished();
            self.outbox.extend(packets.into_iter().map(|packet| {
                ClientToServer::Action(PlayerAction::Schematic(SchematicRequest::Blob(packet)))
            }));
            if finished {
                self.schematics.upload = None;
            }
        }
        let mut credits = Vec::new();
        let mut arrived = Vec::new();
        self.schematics.downloads.retain(|id, download| {
            if let Some(credit) = download.take_credit() {
                credits.push(credit);
            }
            match download.finish() {
                None => true,
                Some(result) => {
                    arrived.push((*id, result));
                    false
                }
            }
        });
        self.outbox.extend(credits.into_iter().map(|packet| {
            ClientToServer::Action(PlayerAction::Schematic(SchematicRequest::Blob(packet)))
        }));
        for (id, result) in arrived {
            match result {
                Ok(bytes) => {
                    let job = Job::spawn(&self.jobs, move || archive::decode(&bytes));
                    self.schematics.decoding.insert(id, job);
                }
                Err(message) => self.notice = message,
            }
        }
        let decoded: Vec<_> = self
            .schematics
            .decoding
            .iter()
            .filter_map(|(id, job)| Some((*id, job.poll()?)))
            .collect();
        for (id, outcome) in decoded {
            self.schematics.decoding.remove(&id);
            match outcome.unwrap_or_else(|_| Err("Decoding the schematic failed".into())) {
                Ok(schematic) if self.schematics.captures.remove(&id) => {
                    self.schematic_captured(Arc::new(schematic))
                }
                Ok(schematic) => self.keep_design(id, Arc::new(schematic)),
                Err(message) => self.notice = message,
            }
        }
        self.start_ready_positioning();
    }

    /// Begin placement preview for an opened positioning once its design is
    /// here.
    fn start_ready_positioning(&mut self) {
        let Some((_, id, _, _)) = self.schematics.positioning.as_ref() else {
            return;
        };
        let Some(schematic) = self.schematics.designs.get(id).cloned() else {
            return;
        };
        let (tag, id, origin, turns) = self.schematics.positioning.take().unwrap();
        self.cancel_world_tools();
        self.schematic_preview
            .begin_positioning(schematic, tag, id, origin, turns);
    }
}
