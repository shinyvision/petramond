//! A client mod's own files: positioned and appending writes, sync, rename,
//! delete, ranged reads, paged listings and stats, all in its storage buckets
//! and all ticketed. Client instances, beside a world or on the shell.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::client::ClientStorageScope;
use crate::legality::prelude::*;

host_domain! {
    /// A client mod's own files, in its storage buckets. Every mutation of one
    /// file lands in submission order, the engine's own records included;
    /// different files proceed in parallel.
    ClientFileCall {
        /// CLIENT: append `bytes` at the end of this mod's file `path` in
        /// `scope`, creating it (and its parent directories) on the first write.
        /// A path breaking [`file_path_problem`](crate::file_path_problem) is
        /// [`HostRet::Err`](crate::HostRet::Err).
        ///
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket), polled with
        /// [`ClientFilePoll`](Self::ClientFilePoll) (`Done { range }` = where
        /// the bytes landed, known only once they have). REFUSED
        /// ([`ErrorCode::Refused`](crate::ErrorCode::Refused)): the `World`
        /// scope on the shell or while a presentation presents, or a path an
        /// unfinished media file writes. A create colliding by case with a
        /// sibling, or a directory at `path`, is found where the write runs:
        /// its ticket's poll answers `Refused`.
        ClientFileAppend {
            scope: ClientStorageScope,
            path: String,
            #[serde(with = "serde_bytes")]
            bytes: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: put `bytes` at `offset` of `path`: a gap past the end fills
        /// with zeros, and `truncate` then cuts the file at `offset +
        /// bytes.len()` (offset 0 with `truncate` replaces the content; empty
        /// `bytes` with `truncate` truncates). Invalidates exactly the ranges it
        /// overlaps in every presentation and engine cache.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket), refused as
        /// [`ClientFileAppend`](Self::ClientFileAppend) is, and also while a
        /// running events log writes `path`.
        ClientFileWrite {
            scope: ClientStorageScope,
            path: String,
            offset: u64,
            #[serde(with = "serde_bytes")]
            bytes: Vec<u8>,
            truncate: bool,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: fsync `path` once every write queued before this has landed.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket).
        ClientFileSync {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: rename `from` to `to`, after every queued write to either:
        /// `from` is synced, renamed, then its directory synced. An existing FILE
        /// at `to` is replaced atomically; a directory never is.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket); its poll answers
        /// `Refused` when `from` is missing, `to` is a directory, or the OS
        /// refused (in its words).
        ClientFileRename {
            scope: ClientStorageScope,
            from: String,
            to: String,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: delete a file, or a directory with everything under it, after
        /// the writes queued under it. What a presentation read from it leaves
        /// the presented world at the call; a presentation never keeps a file
        /// from being deleted. → [`HostRet::Ticket`](crate::HostRet::Ticket).
        ClientFileDelete {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: read `len` bytes at `offset` of `path`. A read sees this
        /// mod's own queued writes, and never waits behind writes it does not
        /// overlap. Bytes past the end are simply not there. A `len` above the
        /// calling instance's
        /// [`guest_memory_max`](crate::ClientEngineFactsData::guest_memory_max)
        /// is [`HostRet::Err`](crate::HostRet::Err): the reply could never be
        /// delivered. → [`HostRet::Ticket`](crate::HostRet::Ticket), answered
        /// `Read(bytes)`.
        ClientFileRead {
            scope: ClientStorageScope,
            path: String,
            offset: u64,
            len: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: one level of directory `dir` (`""` = the bucket's root),
        /// sorted by name, starting after `after`: entries while the encoded
        /// answer stays within `max_bytes`, and at least one while any remain.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket), answered
        /// `Listing { entries, more }`.
        ClientFileList {
            scope: ClientStorageScope,
            dir: String,
            after: Option<String>,
            max_bytes: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: how a file ticket stands — every file ticket polls here
        /// (writes, reads, listings, renames, deletes and state records).
        /// → [`HostRet::ClientFilePolled`](crate::HostRet::ClientFilePolled):
        /// `None` = not finished; `Some` answers and CONSUMES the ticket. A
        /// ticket this instance never received, or one already consumed, is
        /// [`HostRet::Err`](crate::HostRet::Err).
        ClientFilePoll {
            ticket: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: `path`'s size, backlog and last write failure, from the
        /// engine's queue bookkeeping.
        /// → [`HostRet::ClientFileStat`](crate::HostRet::ClientFileStat): `None`
        /// = no such file (a directory answers `None`; list it instead).
        ClientFileStat {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: show this mod's file or directory `path` in the OS file
        /// manager. → [`HostRet::Bool`](crate::HostRet::Bool): `false` = no such
        /// path, or no file manager.
        ClientFileReveal {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: open the OS folder picker, titled `title`, for this mod's
        /// folder slot `folder` ([`ClientStorageScope::Chosen`]). It starts at
        /// the slot's current folder, else the player's videos folder. What the
        /// player picks is remembered for the slot across runs.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket), answered
        /// `Folder(Some(info))`, or `Folder(None)` when the player cancels.
        /// REFUSED ([`ErrorCode::Refused`](crate::ErrorCode::Refused)): a picker
        /// is already open, or this build has none.
        ClientFolderChoose {
            folder: u32,
            title: String,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: the folder chosen for slot `folder`.
        /// → [`HostRet::ClientFolder`](crate::HostRet::ClientFolder): `None` =
        /// none chosen, or it is gone.
        ClientFolderState {
            folder: u32,
        } => legal(CLIENT_SHELL, Any, Read),
    }
}
