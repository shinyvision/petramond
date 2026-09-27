use mod_api::capture::{ClientCapturedSession, ClientStateTicketData};
use mod_api::{ClientCaptureCall, ClientFileAnswer, HostRet};

use super::files::{checked_path, write_refusal};
use crate::capture::moment::Moment;
use crate::modding::client::files;
use crate::modding::client::scope as client_scope;
use crate::modding::client::state::ClientStoreData;

pub(super) fn handle(client: &mut ClientStoreData, call: ClientCaptureCall) -> HostRet {
    let ClientCaptureCall::ClientWorldStateWrite {
        scope,
        path,
        select,
        kinds,
        envelopes,
    } = call
    else {
        return HostRet::invalid(format!(
            "non-state call {call:?} mis-routed to the state handler (host bug)"
        ));
    };
    if let Err(bug) = checked_path("ClientWorldStateWrite", &path).and_then(|()| {
        envelopes
            .as_deref()
            .map_or(Ok(()), |e| checked_path("ClientWorldStateWrite", e))
    }) {
        return bug;
    }
    if let Some(kind) = kinds.iter().flatten().find(|kind| !kind.is_state()) {
        return HostRet::invalid(format!(
            "ClientWorldStateWrite: {kind:?} is not a state piece kind"
        ));
    }
    if let Some(refused) = write_refusal(client, scope) {
        return HostRet::refused(refused);
    }
    let located = files::locate(client, scope, &path).map(|file| {
        let envelopes = envelopes
            .as_deref()
            .and_then(|e| files::locate(client, scope, e));
        (file, envelopes)
    });
    let Some((file, envelopes)) = located else {
        return HostRet::invalid(
            "ClientWorldStateWrite: scope World needs a world, and this instance runs on the \
             shell with none"
                .into(),
        );
    };
    let (desk, local_player) = {
        let presented = client.presented.lock();
        (presented.capture.clone(), presented.local_player)
    };
    let moment = desk.lock().moment.clone();
    let taken = client_scope::with_active(|world| {
        let moment = moment.unwrap_or_else(|| {
            Moment::new(ClientCapturedSession {
                seed: world.data().seed,
                local_player: local_player.unwrap_or(mod_api::PlayerId(0)),
                mods: Vec::new(),
            })
        });
        let snap = crate::capture::state::take(world, &moment, &select, kinds.as_deref());
        (snap, std::sync::Arc::clone(world.job_pool()))
    });
    let Some((snap, jobs)) = taken else {
        return HostRet::invalid("no client replica is active".into());
    };
    let revision = snap.revision;
    let sink = client.files.issue();
    let ticket = sink.ticket();
    client.files.touch(&file);
    if let Some(envelopes) = &envelopes {
        client.files.touch(envelopes);
    }
    if snap.is_empty() {
        let len = files::stat(&file).map_or(0, |info| info.len);
        sink.finish(Ok(ClientFileAnswer::Done {
            range: Some([len, 0]),
            envelope: None,
        }));
    } else if let Err(refused) =
        crate::capture::state::submit(snap, file, envelopes, &jobs, move |done| sink.finish(done))
    {
        client.files.withdraw(ticket);
        return HostRet::refused(refused);
    }
    HostRet::ClientStateTicket(ClientStateTicketData {
        write: ticket,
        revision,
    })
}
