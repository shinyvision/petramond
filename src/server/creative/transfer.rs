//! Designs crossing the wire for operator edits: a paste names its design by
//! digest and holds its place in the queue while the archive arrives, and a
//! selection capture goes back to the client as a blob.

use std::sync::Arc;
use std::thread::JoinHandle;

use super::Pending;
use crate::schematic::store::{digest, Asset, Digest, Lookup};
use crate::schematic::{archive, CreativeReply, Schematic, SelectionBox};
use crate::server::game::ServerGame;
use crate::server::schematics::Wanted;

/// Where a queued paste's design stands.
pub(in crate::server) enum Design {
    /// The world holds it.
    Held(Digest),
    /// The client is uploading it.
    Arriving(Digest),
    Decoding(JoinHandle<Result<Schematic, String>>),
}

/// A design ready to paste.
pub(super) enum Decoded {
    Asset(Arc<Asset>),
    Uploaded(Schematic),
}

impl Decoded {
    pub(super) fn schematic(&self) -> &Schematic {
        match self {
            Self::Asset(asset) => &asset.schematic,
            Self::Uploaded(schematic) => schematic,
        }
    }
}

impl ServerGame {
    /// Queue a paste of `digest`. It takes its queue slot now, so an edit
    /// requested after it cannot overtake a design still arriving.
    pub(in crate::server) fn request_placement(
        &mut self,
        s: usize,
        digest: Digest,
        origin: [i32; 3],
        turns: u8,
    ) {
        if !self.may_edit(s) {
            return self.sessions[s]
                .creative
                .refuse("Creative tools require creative mode".into());
        }
        let design = if self.world.schematics().store.contains(&digest) {
            Design::Held(digest)
        } else if self.sessions[s].creative.has_room()
            && self.sessions[s].schematic.want(Wanted::Edit, digest)
        {
            Design::Arriving(digest)
        } else {
            return self.sessions[s]
                .creative
                .refuse("A schematic is still being transferred".into());
        };
        self.sessions[s].creative.try_enqueue(Pending::Placement {
            design,
            origin,
            turns,
        });
    }

    /// An uploaded design's bytes arrived whole: decode them for the paste
    /// waiting on them.
    pub(in crate::server) fn placement_design_arrived(
        &mut self,
        s: usize,
        arrived: Digest,
        bytes: Vec<u8>,
    ) {
        for pending in &mut self.sessions[s].creative.pending {
            if let Pending::Placement { design, .. } = pending {
                if matches!(design, Design::Arriving(d) if *d == arrived) {
                    *design = Design::Decoding(std::thread::spawn(move || archive::decode(&bytes)));
                    return;
                }
            }
        }
    }

    /// The design at the head of the queue: `None` while it is still on its
    /// way.
    pub(super) fn poll_design(
        &mut self,
        s: usize,
        design: &mut Design,
    ) -> Option<Result<Decoded, String>> {
        match design {
            Design::Held(digest) => match self.world.schematics_mut().store.lookup(digest) {
                Lookup::Loading => None,
                Lookup::Ready(asset) => Some(Ok(Decoded::Asset(asset))),
                Lookup::Missing => Some(Err("The world does not hold that schematic".into())),
                Lookup::Failed(message) => Some(Err(message)),
            },
            Design::Arriving(digest) => (!self.sessions[s].schematic.wants(digest))
                .then(|| Err("The schematic did not arrive".into())),
            Design::Decoding(job) if !job.is_finished() => None,
            Design::Decoding(_) => {
                let Design::Decoding(job) = std::mem::replace(design, Design::Held([0; 32])) else {
                    unreachable!()
                };
                Some(
                    job.join()
                        .unwrap_or_else(|_| Err("Schematic decoding failed".into()))
                        .map(Decoded::Uploaded),
                )
            }
        }
    }

    pub(super) fn begin_capture(
        &mut self,
        s: usize,
        name: String,
        regions: Vec<SelectionBox>,
        include_air: bool,
    ) -> Result<(), String> {
        let public_only = !self.sessions[s].player.abilities().edits_cells;
        let capture = crate::world::schematic::capture::Capture::new(
            &self.world,
            name,
            regions,
            include_air,
            public_only,
        )?;
        self.sessions[s].creative.capture = Some(std::thread::spawn(move || {
            let bytes = archive::encode_bare(&capture.run()?)?;
            Ok((digest(&bytes), bytes.into()))
        }));
        Ok(())
    }

    /// Send a finished capture to the client. Returns whether the queue may
    /// advance: a capture still running holds the requests behind it.
    pub(super) fn poll_capture(&mut self, s: usize) -> bool {
        let session = &mut self.sessions[s];
        let Some(job) = session.creative.capture.as_ref() else {
            return true;
        };
        if !job.is_finished() {
            return false;
        }
        let captured = session
            .creative
            .capture
            .take()
            .unwrap()
            .join()
            .unwrap_or_else(|_| Err("Schematic capture failed".into()));
        match captured {
            Ok((digest, bytes)) => {
                session
                    .creative
                    .replies
                    .push(CreativeReply::Captured { digest });
                session.schematic.send(digest, bytes);
            }
            Err(message) => session.creative.refuse(message),
        }
        true
    }
}
