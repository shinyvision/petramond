//! This mod's FILES, in its `Pack` bucket (everywhere) or the presented
//! world's `World` bucket: named and laid out as the mod likes, appended to,
//! rewritten in place, read back in ranges, listed a page at a time.
//!
//! Every operation but [`client_file_stat`] and [`client_file_reveal`] is
//! ticketed: it answers at once, and [`client_file_poll`] says how it ended.
//! A path breaking [`crate::file_path_problem`] disables the mod, so check a
//! name a player typed before passing it.

use core::ops::Deref;

use mod_api::{
    ClientFileAnswer, ClientFileEntry, ClientFileInfo, ClientFolderInfo, ClientStorageScope,
    HostCall, HostError, HostRet,
};

use crate::__rt::{host_call_reply, host_fn, recoverable, try_host_fn, Answer, Reply};

/// Bytes a read delivered. In the guest they live in the host's reply
/// allocation itself, so a read of N bytes costs N bytes (and a few) of
/// memory, never 2N. Derefs to `[u8]`, and frees on drop.
pub struct Bytes(Repr);

enum Repr {
    Reply {
        reply: Reply,
        start: usize,
        len: usize,
    },
    Owned(Vec<u8>),
}

impl Bytes {
    pub fn to_vec(&self) -> Vec<u8> {
        self.deref().to_vec()
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match &self.0 {
            Repr::Reply { reply, start, len } => &reply.bytes()[*start..*start + *len],
            Repr::Owned(bytes) => bytes,
        }
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl core::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Bytes({} bytes)", self.len())
    }
}

/// How a file ticket ended ([`client_file_poll`]).
#[derive(Debug)]
pub enum FileAnswer {
    /// A write, sync, rename, delete or engine record finished. `range`: where
    /// an append, a write or an engine record landed. `envelope`: a state
    /// record's envelope. Both absolute `[offset, len]`.
    Done {
        range: Option<[u64; 2]>,
        envelope: Option<[u64; 2]>,
    },
    /// A read's bytes: those that exist in the range asked for.
    Read(Bytes),
    /// One page of a listing, sorted by name; `more` = continue after the
    /// last name.
    Listing {
        entries: Vec<ClientFileEntry>,
        more: bool,
    },
    /// A folder choice: the folder now chosen, or `None` = cancelled.
    Folder(Option<ClientFolderInfo>),
}

impl From<ClientFileAnswer> for FileAnswer {
    fn from(answer: ClientFileAnswer) -> Self {
        match answer {
            ClientFileAnswer::Done { range, envelope } => FileAnswer::Done { range, envelope },
            ClientFileAnswer::Read(bytes) => FileAnswer::Read(Bytes(Repr::Owned(bytes))),
            ClientFileAnswer::Listing { entries, more } => FileAnswer::Listing { entries, more },
            ClientFileAnswer::Folder(info) => FileAnswer::Folder(info),
        }
    }
}

/// Where a `Read` answer's bytes lie in its encoded reply: everything before
/// them is a fixed prefix and the length, so they are found without decoding.
fn read_span(reply: &[u8]) -> Option<(usize, usize)> {
    let template = HostRet::ClientFilePolled(Some(ClientFileAnswer::Read(Vec::new())));
    let encoded = mod_api::encode(&template).ok()?;
    // The template ends in the empty byte string's one-byte length.
    let prefix = &encoded[..encoded.len() - 1];
    let rest = reply.strip_prefix(prefix)?;
    let (mut len, mut shift, mut used) = (0u64, 0u32, 0usize);
    loop {
        let byte = *rest.get(used)?;
        used += 1;
        len |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    let len = usize::try_from(len).ok()?;
    (rest.len() == used + len).then_some((prefix.len() + used, len))
}

/// CLIENT: how a file ticket stands — every file ticket polls here. `None` =
/// not finished; `Some` answers and CONSUMES the ticket, `Some(Err)` with why
/// it failed ([`mod_api::ErrorCode::Refused`]). Polling a ticket this mod
/// never received, or one already consumed, disables the mod.
///
/// Written by hand, not through `host_fn!`: a `Read` answer keeps the host's
/// reply allocation as its buffer instead of decoding a copy of it.
pub fn client_file_poll(ticket: u64) -> Option<Result<FileAnswer, HostError>> {
    let ret = match host_call_reply(&HostCall::from(mod_api::calls::ClientFilePoll { ticket })) {
        Answer::Native(ret) => ret,
        Answer::Guest(reply) => match read_span(reply.bytes()) {
            Some((start, len)) => {
                return Some(Ok(FileAnswer::Read(Bytes(Repr::Reply {
                    reply,
                    start,
                    len,
                }))))
            }
            None => reply.decode(),
        },
    };
    match recoverable("ClientFilePoll", ret) {
        Ok(HostRet::ClientFilePolled(answer)) => answer.map(|answer| Ok(answer.into())),
        Err(failed) => Some(Err(failed)),
        Ok(other) => panic!("ClientFilePoll returned {other:?}"),
    }
}

try_host_fn! {
    /// CLIENT: append `bytes` at the end of `path`, creating it and its
    /// directories. Every mutation of one file lands in submission order.
    /// `Ok(ticket)` (`Done { range }` = where they landed); `Err` = refused.
    pub fn client_file_append(scope: ClientStorageScope, path: &str, bytes: Vec<u8>)
        -> u64
        => ClientFileAppend { scope, path: path.into(), bytes } => Ticket
}

try_host_fn! {
    /// CLIENT: put `bytes` at `offset` of `path` (a gap fills with zeros);
    /// `truncate` then cuts the file at `offset + bytes.len()`.
    pub fn client_file_write(
        scope: ClientStorageScope,
        path: &str,
        offset: u64,
        bytes: Vec<u8>,
        truncate: bool,
    ) -> u64
        => ClientFileWrite { scope, path: path.into(), offset, bytes, truncate } => Ticket
}

try_host_fn! {
    /// CLIENT: fsync `path` once the writes queued before this have landed.
    pub fn client_file_sync(scope: ClientStorageScope, path: &str) -> u64
        => ClientFileSync { scope, path: path.into() } => Ticket
}

try_host_fn! {
    /// CLIENT: rename `from` to `to` after the writes queued to either; an
    /// existing file at `to` is replaced atomically, a directory never is.
    pub fn client_file_rename(scope: ClientStorageScope, from: &str, to: &str)
        -> u64
        => ClientFileRename { scope, from: from.into(), to: to.into() } => Ticket
}

try_host_fn! {
    /// CLIENT: delete a file, or a directory with everything under it.
    pub fn client_file_delete(scope: ClientStorageScope, path: &str) -> u64
        => ClientFileDelete { scope, path: path.into() } => Ticket
}

try_host_fn! {
    /// CLIENT: read `len` bytes at `offset` of `path`; answered `Read`. A read
    /// sees this mod's own queued writes. `len` above this instance's
    /// `guest_memory_max` ([`crate::client_engine_facts`]) disables the mod.
    pub fn client_file_read(scope: ClientStorageScope, path: &str, offset: u64, len: u64)
        -> u64
        => ClientFileRead { scope, path: path.into(), offset, len } => Ticket
}

try_host_fn! {
    /// CLIENT: one level of `dir` (`""` = the bucket's root), sorted by name,
    /// after `after`, within `max_bytes` of answer; answered `Listing`.
    pub fn client_file_list(
        scope: ClientStorageScope,
        dir: &str,
        after: Option<&str>,
        max_bytes: u64,
    ) -> u64
        => ClientFileList { scope, dir: dir.into(), after: after.map(Into::into), max_bytes }
        => Ticket
}

host_fn! {
    /// CLIENT: `path`'s size, backlog (`len - written`) and last write
    /// failure; `None` = no such file (a directory answers `None`).
    pub fn client_file_stat(scope: ClientStorageScope, path: &str) -> Option<ClientFileInfo>
        => ClientFileStat { scope, path: path.into() } => ClientFileStat
}

host_fn! {
    /// CLIENT: show this mod's file or directory in the OS file manager.
    /// `false` = no such path, or no file manager.
    pub fn client_file_reveal(scope: ClientStorageScope, path: &str) -> bool
        => ClientFileReveal { scope, path: path.into() } => Bool
}

try_host_fn! {
    /// CLIENT: open the OS folder picker for this mod's folder slot `folder`
    /// (`ClientStorageScope::Chosen(folder)`); what the player picks is
    /// remembered. Answered `Folder(Some(info))`, or `Folder(None)` when the
    /// player cancels; `Err` = refused (a picker is already open, or this
    /// build has none).
    pub fn client_folder_choose(folder: u32, title: &str) -> u64
        => ClientFolderChoose { folder, title: title.into() } => Ticket
}

host_fn! {
    /// CLIENT: the folder chosen for slot `folder`; `None` = none chosen, or
    /// it is gone.
    pub fn client_folder_state(folder: u32) -> Option<ClientFolderInfo>
        => ClientFolderState { folder } => ClientFolder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_answer_is_found_in_its_reply_without_decoding() {
        let bytes: Vec<u8> = (0..300u16).map(|i| i as u8).collect();
        let reply = mod_api::encode(&HostRet::ClientFilePolled(Some(ClientFileAnswer::Read(
            bytes.clone(),
        ))))
        .unwrap();
        let (start, len) = read_span(&reply).expect("a read answer");
        assert_eq!(&reply[start..start + len], &bytes[..]);
        let done = mod_api::encode(&HostRet::ClientFilePolled(Some(ClientFileAnswer::Done {
            range: None,
            envelope: None,
        })))
        .unwrap();
        assert_eq!(read_span(&done), None);
        let refused = mod_api::encode(&HostRet::refused("gone")).unwrap();
        assert_eq!(read_span(&refused), None);
    }
}
