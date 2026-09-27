use crate::capture::{ClientPieceKind, ClientStateSelect};
use crate::client::ClientStorageScope;
use crate::legality::prelude::*;

host_domain! {
    ClientCaptureCall {
        ClientWorldStateWrite {
            scope: ClientStorageScope,
            path: String,
            select: ClientStateSelect,
            kinds: Option<Vec<ClientPieceKind>>,
            envelopes: Option<String>,
        } => legal(CLIENT, Any, Write),
        ClientWorldEventsBegin {
            scope: ClientStorageScope,
            path: String,
            envelopes: Option<String>,
        } => legal(CLIENT, Any, Write),
        ClientWorldEventsEnd {
            events: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientWorldEventsPoll {
            events: u64,
        } => legal(CLIENT_SHELL, Any, Read),
    }
}
