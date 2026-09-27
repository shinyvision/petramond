//! Presenting a world from mod-file byte ranges, as a client instance drives
//! it: each call validates, answers from the presentation desk, and leaves a
//! request the client carries out in issue order at its next frame.

use mod_api::{ClientFileRange, ClientFileRanges, ClientPose, ClientPresentationCall, HostRet};

use super::files::checked_path;
use crate::capture::source::{FileRanges, SourceFile};
use crate::modding::client::files;
use crate::modding::client::state::ClientStoreData;

const NOT_OWNER: &str = "this mod has no open presentation";

fn finite_pose(pose: &ClientPose) -> bool {
    pose.pos.iter().all(|v| v.is_finite()) && pose.yaw.is_finite() && pose.pitch.is_finite()
}

/// The file `scope`/`path` names, as the presentation reads it.
fn source(
    client: &ClientStoreData,
    call: &str,
    scope: mod_api::ClientStorageScope,
    path: &str,
) -> Result<SourceFile, HostRet> {
    checked_path(call, path)?;
    files::locate(client, scope, path)
        .map(SourceFile::new)
        .ok_or_else(|| {
            HostRet::invalid(format!(
                "{call}: scope World needs a world, and this instance runs on the shell with none"
            ))
        })
}

fn resolved(
    client: &ClientStoreData,
    call: &str,
    ranges: Vec<ClientFileRanges>,
) -> Result<Vec<FileRanges>, HostRet> {
    ranges
        .into_iter()
        .map(|r| {
            if let Some([at, len]) = r
                .ranges
                .iter()
                .find(|[at, len]| at.checked_add(*len).is_none())
            {
                return Err(HostRet::invalid(format!(
                    "{call}: range [{at}, {len}] of '{}' ends past the largest offset",
                    r.path
                )));
            }
            Ok(FileRanges {
                file: source(client, call, r.scope, &r.path)?,
                ranges: r.ranges,
            })
        })
        .collect()
}

fn one_range(
    client: &ClientStoreData,
    call: &str,
    r: ClientFileRange,
) -> Result<FileRanges, HostRet> {
    if r.offset.checked_add(r.len).is_none() {
        return Err(HostRet::invalid(format!(
            "{call}: range [{}, {}] of '{}' ends past the largest offset",
            r.offset, r.len, r.path
        )));
    }
    Ok(FileRanges {
        file: source(client, call, r.scope, &r.path)?,
        ranges: vec![[r.offset, r.len]],
    })
}

pub(super) fn handle(
    client: &mut ClientStoreData,
    mod_id: &str,
    call: ClientPresentationCall,
) -> HostRet {
    match call {
        ClientPresentationCall::ClientPresentationOpen {
            tables,
            seed,
            mods,
            viewer,
        } => {
            let tables = match one_range(client, "ClientPresentationOpen", tables) {
                Ok(t) => t,
                Err(bug) => return bug,
            };
            if viewer.as_ref().is_some_and(|pose| !finite_pose(pose)) {
                return HostRet::invalid(
                    "ClientPresentationOpen: viewer pose must be finite".into(),
                );
            }
            let shell = client.shell;
            let mut presented = client.presented.lock();
            let refusal = match (&presented.context, presented.presentation.owner()) {
                (_, Some(owner)) if owner != mod_id => {
                    Some(format!("the presentation on screen is '{owner}''s"))
                }
                (mod_api::ClientContext::Presentation { owner }, _) if owner != mod_id => {
                    Some(format!("the presentation on screen is '{owner}''s"))
                }
                (mod_api::ClientContext::Presentation { .. }, _) => None,
                _ if shell => None,
                _ => Some(
                    "a presentation opens only from the title screen, never over a running world"
                        .into(),
                ),
            };
            let desk = &mut presented.presentation;
            match refusal {
                Some(why) => {
                    desk.refuse(why);
                    HostRet::Bool(false)
                }
                None => {
                    desk.open(mod_id, tables, seed, mods, viewer);
                    HostRet::Bool(true)
                }
            }
        }
        ClientPresentationCall::ClientPresentationApply { state, events, at } => {
            if !at.is_finite() {
                return HostRet::invalid("ClientPresentationApply: at must be finite".into());
            }
            let (state, events) = match resolved(client, "ClientPresentationApply", state)
                .and_then(|s| Ok((s, resolved(client, "ClientPresentationApply", events)?)))
            {
                Ok(both) => both,
                Err(bug) => return bug,
            };
            let mut presented = client.presented.lock();
            let desk = &mut presented.presentation;
            if !desk.owns(mod_id) {
                return HostRet::refused(NOT_OWNER);
            }
            let id = files::issue_id();
            desk.apply(id, mod_id, state, events, at);
            HostRet::Ticket(id)
        }
        ClientPresentationCall::ClientPresentationCancel { apply } => {
            match client.presented.lock().presentation.cancel(apply, mod_id) {
                Some(cancelled) => HostRet::Bool(cancelled),
                None => HostRet::invalid(format!(
                    "ClientPresentationCancel: apply {apply} was never issued to this instance"
                )),
            }
        }
        ClientPresentationCall::ClientPresentationQueue { events } => {
            let events = match resolved(client, "ClientPresentationQueue", events) {
                Ok(e) => e,
                Err(bug) => return bug,
            };
            let mut presented = client.presented.lock();
            let desk = &mut presented.presentation;
            if !desk.owns(mod_id) {
                return HostRet::Bool(false);
            }
            desk.queue(events);
            HostRet::Bool(true)
        }
        ClientPresentationCall::ClientPresentationTime { at } => {
            if !at.is_finite() {
                return HostRet::invalid("ClientPresentationTime: at must be finite".into());
            }
            let mut presented = client.presented.lock();
            let desk = &mut presented.presentation;
            HostRet::Bool(desk.owns(mod_id) && desk.time(at))
        }
        ClientPresentationCall::ClientPresentationViewer { pose, flying } => {
            if !finite_pose(&pose) {
                return HostRet::invalid("ClientPresentationViewer: pose must be finite".into());
            }
            let mut presented = client.presented.lock();
            let desk = &mut presented.presentation;
            if !desk.owns(mod_id) {
                return HostRet::Bool(false);
            }
            desk.viewer(pose, flying);
            HostRet::Bool(true)
        }
        ClientPresentationCall::ClientPresentationState => {
            HostRet::ClientPresentationState(Box::new(client.presented.lock().presentation.state()))
        }
        ClientPresentationCall::ClientPresentationClose => {
            let mut presented = client.presented.lock();
            if presented.presentation.owns(mod_id) {
                presented.presentation.close();
            }
            HostRet::Unit
        }
    }
}
