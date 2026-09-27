use crate::capture::ClientPose;
use crate::files::{ClientFileRange, ClientFileRanges};
use crate::legality::prelude::*;

host_domain! {
    ClientPresentationCall {
        ClientPresentationOpen {
            tables: ClientFileRange,
            seed: u32,
            mods: Vec<String>,
            viewer: Option<ClientPose>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientPresentationApply {
            state: Vec<ClientFileRanges>,
            events: Vec<ClientFileRanges>,
            at: f64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientPresentationCancel {
            apply: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientPresentationQueue {
            events: Vec<ClientFileRanges>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientPresentationTime {
            at: f64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientPresentationViewer {
            pose: ClientPose,
            flying: bool,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientPresentationState => legal(CLIENT_SHELL, Any, Read),
        ClientPresentationClose => legal(CLIENT_SHELL, Any, Write),
    }
}
