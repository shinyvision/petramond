//! Schematic sharing with each connected client: open choices and
//! positionings a mod asked for, archive uploads the world does not hold yet,
//! archive downloads a client asked for, and the anchored ghosts each client
//! should draw. Requests land at message time; streams and ghosts advance
//! once per tick.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc;

use petramond_math::math::IVec3;

use super::game::ServerGame;
use crate::events::PostEvent;
use crate::schematic::share::{
    BlobPacket, BlobReceiver, BlobSender, GhostPlacement, SchematicNotice, SchematicRequest,
};
use crate::schematic::store::{Digest, Finished};

/// How many downloads one client may have queued at once.
const FETCH_QUEUE: usize = 8;

/// What the server asked a client's archive for.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Wanted {
    /// The open choice with this tag: the archive becomes a world asset.
    Choice(String),
    /// An operator's paste: used once, never kept.
    Edit,
}

/// An archive read off the tick thread, or why it could not be.
type ArchiveBytes = Result<std::sync::Arc<[u8]>, String>;

#[derive(Default)]
pub struct SchematicSession {
    choose: Option<String>,
    position: Option<(String, Digest)>,
    /// The archive the server asked this client to upload, for which choice.
    wanted: Option<(Wanted, Digest)>,
    upload: Option<BlobReceiver>,
    /// Uploads handed to the store, awaiting publication, by choice.
    publishing: Vec<(String, Digest)>,
    fetches: VecDeque<Digest>,
    reading: Option<(Digest, mpsc::Receiver<ArchiveBytes>)>,
    download: Option<BlobSender>,
    /// Blobs of the server's own making, sent ahead of fetched assets.
    sending: VecDeque<BlobSender>,
    /// The ghosts as this client last heard them.
    ghosts_sent: BTreeMap<String, GhostPlacement>,
    notices: Vec<SchematicNotice>,
}

impl SchematicSession {
    pub fn take_notices(&mut self) -> Vec<SchematicNotice> {
        std::mem::take(&mut self.notices)
    }

    /// Ask the client for the archive of `digest`; `false` while another
    /// upload is still wanted or arriving.
    pub(super) fn want(&mut self, wanted: Wanted, digest: Digest) -> bool {
        if self.wanted.is_some() || self.upload.is_some() {
            return false;
        }
        self.wanted = Some((wanted, digest));
        self.notices.push(SchematicNotice::Want { digest });
        true
    }

    /// Whether the upload of `digest` is still wanted or arriving.
    pub(super) fn wants(&self, digest: &Digest) -> bool {
        self.wanted.as_ref().is_some_and(|(_, d)| d == digest)
    }

    /// Stream `bytes` to the client as the blob `digest`.
    pub(super) fn send(&mut self, digest: Digest, bytes: std::sync::Arc<[u8]>) {
        self.sending.push_back(BlobSender::new(digest, bytes));
    }

    /// Whether a mod's choice is open on this client.
    pub fn choice_open(&self) -> bool {
        self.choose.is_some()
    }
}

impl ServerGame {
    /// A client's schematic request, at message time.
    pub(super) fn apply_schematic_request(&mut self, s: usize, request: SchematicRequest) {
        let player = self.sessions[s].id;
        match request {
            SchematicRequest::Chosen { tag, digest } => {
                if self.sessions[s].schematic.choose.as_ref() != Some(&tag) {
                    return;
                }
                if self.world.schematics().store.contains(&digest) {
                    self.sessions[s].schematic.choose = None;
                    self.bus.emit(PostEvent::SchematicChosen {
                        player,
                        tag,
                        asset: digest,
                    });
                } else {
                    self.sessions[s].schematic.want(Wanted::Choice(tag), digest);
                }
            }
            SchematicRequest::Positioned {
                tag,
                digest,
                origin,
                turns,
            } => {
                if self.sessions[s].schematic.position != Some((tag.clone(), digest)) {
                    return;
                }
                let Some(pivot) = self.positioned_pivot(&digest, origin, turns) else {
                    return;
                };
                let eye = super::movement::reach_eye(&self.sessions[s]);
                if (petramond_math::world_pos::WorldPos::block_center(pivot) - eye).length()
                    > crate::schematic::PLACEMENT_REACH + 2.0
                {
                    self.sessions[s]
                        .schematic
                        .notices
                        .push(SchematicNotice::Refused {
                            message: "Place the schematic within reach".into(),
                        });
                    return;
                }
                self.sessions[s].schematic.position = None;
                self.bus.emit(PostEvent::SchematicPositioned {
                    player,
                    tag,
                    asset: digest,
                    origin: IVec3::from_array(origin),
                    turns: turns % 4,
                });
            }
            SchematicRequest::Fetch { digest } => {
                let session = &mut self.sessions[s].schematic;
                if session.fetches.len() < FETCH_QUEUE && !session.fetches.contains(&digest) {
                    session.fetches.push_back(digest);
                }
            }
            SchematicRequest::Blob(packet) => self.receive_schematic_blob(s, packet),
        }
    }

    /// The world cell under the design's centred bottom pivot, when the design
    /// is decoded and the transform lies inside the world.
    fn positioned_pivot(&mut self, digest: &Digest, origin: [i32; 3], turns: u8) -> Option<IVec3> {
        let crate::schematic::store::Lookup::Ready(asset) =
            self.world.schematics_mut().store.lookup(digest)
        else {
            return None;
        };
        let size = asset.schematic.rotated_size(turns);
        let end: Option<Vec<i32>> = (0..3).map(|i| origin[i].checked_add(size[i] - 1)).collect();
        let end = end?;
        (petramond_world::border::contains_column(origin[0], origin[2])
            && petramond_world::border::contains_column(end[0], end[2]))
        .then(|| IVec3::from_array(origin) + asset.schematic.placement_pivot(turns))
    }

    fn receive_schematic_blob(&mut self, s: usize, packet: BlobPacket) {
        let session = &mut self.sessions[s].schematic;
        match packet {
            BlobPacket::Begin { digest, .. } => {
                let expected = session.wanted.as_ref().map(|(_, d)| *d);
                if session.upload.is_some() || expected != Some(digest) {
                    return;
                }
                match BlobReceiver::begin(
                    &packet,
                    digest,
                    crate::schematic::archive::MAX_ARCHIVE_BYTES,
                ) {
                    Ok(receiver) => session.upload = Some(receiver),
                    Err(message) => session.notices.push(SchematicNotice::Refused { message }),
                }
            }
            BlobPacket::Data { digest, .. } => {
                let Some(upload) = session.upload.as_mut().filter(|u| u.digest() == digest) else {
                    return;
                };
                if let Err(message) = upload.receive(packet) {
                    session.upload = None;
                    session.wanted = None;
                    session
                        .notices
                        .push(SchematicNotice::Blob(BlobPacket::Cancel { digest }));
                    session.notices.push(SchematicNotice::Refused { message });
                }
            }
            BlobPacket::Credit { digest, count } => {
                if let Some(download) = session.download.as_mut().filter(|d| d.digest() == digest) {
                    if !download.credit(count) {
                        session.download = None;
                    }
                }
            }
            BlobPacket::Cancel { digest } => {
                if session
                    .upload
                    .as_ref()
                    .is_some_and(|u| u.digest() == digest)
                {
                    session.upload = None;
                    session.wanted = None;
                }
                if session
                    .download
                    .as_ref()
                    .is_some_and(|d| d.digest() == digest)
                {
                    session.download = None;
                }
            }
        }
    }

    /// Mod requests that open a choice or a positioning on a client.
    pub(super) fn open_schematic_choice(&mut self, player: crate::player::PlayerId, tag: String) {
        if let Some(sess) = self.sessions.iter_mut().find(|sess| sess.id == player) {
            sess.schematic.choose = Some(tag.clone());
            sess.schematic.notices.push(SchematicNotice::Choose { tag });
        }
    }

    pub(super) fn open_schematic_position(
        &mut self,
        player: crate::player::PlayerId,
        tag: String,
        digest: Digest,
        origin: Option<IVec3>,
        turns: u8,
    ) {
        if let Some(sess) = self.sessions.iter_mut().find(|sess| sess.id == player) {
            sess.schematic.position = Some((tag.clone(), digest));
            sess.schematic.notices.push(SchematicNotice::Position {
                tag,
                digest,
                origin: origin.map(|o| o.to_array()),
                turns,
            });
        }
    }

    /// Advance every session's streams and ghosts: once per tick.
    pub(super) fn tick_schematics(&mut self) {
        for finished in self.world.schematics_mut().store.poll() {
            let Finished::Published(result) = finished;
            for sess in &mut self.sessions {
                let session = &mut sess.schematic;
                let done: Vec<_> = match &result {
                    Ok(digest) => session
                        .publishing
                        .iter()
                        .filter(|(_, d)| d == digest)
                        .cloned()
                        .collect(),
                    Err(_) => Vec::new(),
                };
                session.publishing.retain(|entry| !done.contains(entry));
                for (tag, digest) in done {
                    if session.choose.as_ref() == Some(&tag) {
                        session.choose = None;
                        self.bus.emit(PostEvent::SchematicChosen {
                            player: sess.id,
                            tag,
                            asset: digest,
                        });
                    }
                }
            }
            if let Err(message) = result {
                log::warn!("schematic import: {message}");
            }
        }
        for s in 0..self.sessions.len() {
            self.tick_schematic_upload(s);
            self.tick_schematic_download(s);
            self.sync_schematic_ghosts(s);
        }
    }

    fn tick_schematic_upload(&mut self, s: usize) {
        let session = &mut self.sessions[s].schematic;
        let Some(upload) = session.upload.as_mut() else {
            return;
        };
        if let Some(credit) = upload.take_credit() {
            session.notices.push(SchematicNotice::Blob(credit));
        }
        let Some(result) = upload.finish() else {
            return;
        };
        let digest = upload.digest();
        session.upload = None;
        let wanted = session.wanted.take();
        match (result, wanted) {
            (Ok(bytes), Some((Wanted::Choice(tag), wanted))) if wanted == digest => {
                session.publishing.push((tag, digest));
                self.world.schematics_mut().store.publish(digest, bytes);
            }
            (Ok(bytes), Some((Wanted::Edit, wanted))) if wanted == digest => {
                self.placement_design_arrived(s, digest, bytes);
            }
            (Err(message), _) => session.notices.push(SchematicNotice::Refused { message }),
            _ => {}
        }
    }

    fn tick_schematic_download(&mut self, s: usize) {
        let session = &mut self.sessions[s].schematic;
        if let Some(download) = session.download.as_mut() {
            let packets = download.packets();
            session
                .notices
                .extend(packets.into_iter().map(SchematicNotice::Blob));
            if download.finished() {
                session.download = None;
            }
            return;
        }
        if let Some(next) = session.sending.pop_front() {
            session.download = Some(next);
            return;
        }
        if let Some((digest, rx)) = session.reading.as_ref() {
            match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => session.reading = None,
                Ok(Ok(bytes)) => {
                    session.download = Some(BlobSender::new(*digest, bytes));
                    session.reading = None;
                }
                Ok(Err(message)) => {
                    session.reading = None;
                    session.notices.push(SchematicNotice::Refused { message });
                }
            }
            return;
        }
        let Some(digest) = session.fetches.pop_front() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let store = &self.world.schematics().store;
        if !store.contains(&digest) {
            session.notices.push(SchematicNotice::Refused {
                message: "The world does not hold that schematic".into(),
            });
            return;
        }
        let reader = store.reader(digest);
        std::thread::spawn(move || {
            let _ = tx.send(reader());
        });
        session.reading = Some((digest, rx));
    }

    fn sync_schematic_ghosts(&mut self, s: usize) {
        let id = self.sessions[s].id;
        let visible: BTreeMap<String, GhostPlacement> = self
            .world
            .schematics()
            .ghosts
            .iter()
            .filter(|(_, g)| g.viewers.is_empty() || g.viewers.contains(&id))
            .map(|(key, g)| (key.clone(), g.placement))
            .collect();
        let session = &mut self.sessions[s].schematic;
        if visible == session.ghosts_sent {
            return;
        }
        for key in session.ghosts_sent.keys() {
            if !visible.contains_key(key) {
                session.notices.push(SchematicNotice::Ghost {
                    key: key.clone(),
                    placement: None,
                });
            }
        }
        for (key, placement) in &visible {
            if session.ghosts_sent.get(key) != Some(placement) {
                session.notices.push(SchematicNotice::Ghost {
                    key: key.clone(),
                    placement: Some(*placement),
                });
            }
        }
        session.ghosts_sent = visible;
    }
}

#[cfg(test)]
mod tests;
