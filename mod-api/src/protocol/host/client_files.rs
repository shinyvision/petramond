use crate::client::ClientStorageScope;
use crate::legality::prelude::*;

host_domain! {
    ClientFileCall {
        ClientFileAppend {
            scope: ClientStorageScope,
            path: String,
            #[serde(with = "serde_bytes")]
            bytes: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFileWrite {
            scope: ClientStorageScope,
            path: String,
            offset: u64,
            #[serde(with = "serde_bytes")]
            bytes: Vec<u8>,
            truncate: bool,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFileSync {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFileRename {
            scope: ClientStorageScope,
            from: String,
            to: String,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFileDelete {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFileRead {
            scope: ClientStorageScope,
            path: String,
            offset: u64,
            len: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFileList {
            scope: ClientStorageScope,
            dir: String,
            after: Option<String>,
            max_bytes: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFilePoll {
            ticket: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientFileStat {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientFileReveal {
            scope: ClientStorageScope,
            path: String,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFolderChoose {
            folder: u32,
            title: String,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientFolderState {
            folder: u32,
        } => legal(CLIENT_SHELL, Any, Read),
    }
}
