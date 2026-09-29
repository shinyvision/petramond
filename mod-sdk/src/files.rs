use core::ops::Deref;

use mod_api::{
    ClientFileAnswer, ClientFileEntry, ClientFileInfo, ClientFolderInfo, ClientStorageScope,
    HostError, HostRet,
};

use crate::__rt::{call, host_fn, recoverable_value, try_host_fn, Reply};

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

#[derive(Debug)]
pub enum FileAnswer {
    Done {
        range: Option<[u64; 2]>,
        envelope: Option<[u64; 2]>,
    },
    Read(Bytes),
    Listing {
        entries: Vec<ClientFileEntry>,
        more: bool,
    },
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

fn read_span(reply: &[u8]) -> Option<(usize, usize)> {
    let template = HostRet::ClientFilePolled(Some(ClientFileAnswer::Read(Vec::new())));
    let encoded = mod_api::encode(&template).ok()?;
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

pub fn client_file_poll(ticket: u64) -> Option<Result<FileAnswer, HostError>> {
    let reply = call(&mod_api::calls::ClientFilePoll { ticket });
    if let Some((start, len)) = read_span(reply.bytes()) {
        return Some(Ok(FileAnswer::Read(Bytes(Repr::Reply {
            reply,
            start,
            len,
        }))));
    }
    match recoverable_value(
        "ClientFilePoll",
        reply.decode_as(mod_api::ret_decode::ClientFilePolled),
    ) {
        Ok(answer) => answer.map(|answer| Ok(answer.into())),
        Err(failed) => Some(Err(failed)),
    }
}

try_host_fn! {
    pub fn client_file_append(scope: ClientStorageScope, path: &str, bytes: Vec<u8>)
        -> u64
        => ClientFileAppend { scope, path: path.into(), bytes } => Ticket
}

try_host_fn! {
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
    pub fn client_file_sync(scope: ClientStorageScope, path: &str) -> u64
        => ClientFileSync { scope, path: path.into() } => Ticket
}

try_host_fn! {
    pub fn client_file_rename(scope: ClientStorageScope, from: &str, to: &str)
        -> u64
        => ClientFileRename { scope, from: from.into(), to: to.into() } => Ticket
}

try_host_fn! {
    pub fn client_file_delete(scope: ClientStorageScope, path: &str) -> u64
        => ClientFileDelete { scope, path: path.into() } => Ticket
}

try_host_fn! {
    pub fn client_file_read(scope: ClientStorageScope, path: &str, offset: u64, len: u64)
        -> u64
        => ClientFileRead { scope, path: path.into(), offset, len } => Ticket
}

try_host_fn! {
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
    pub fn client_file_stat(scope: ClientStorageScope, path: &str) -> Option<ClientFileInfo>
        => ClientFileStat { scope, path: path.into() } => ClientFileStat
}

host_fn! {
    pub fn client_file_reveal(scope: ClientStorageScope, path: &str) -> bool
        => ClientFileReveal { scope, path: path.into() } => Bool
}

try_host_fn! {
    pub fn client_folder_choose(folder: u32, title: &str) -> u64
        => ClientFolderChoose { folder, title: title.into() } => Ticket
}

host_fn! {
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
