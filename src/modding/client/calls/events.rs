use mod_api::{ClientCaptureCall, HostRet};

use super::files::{checked_path, write_refusal};
use crate::modding::client::files;
use crate::modding::client::state::ClientStoreData;

pub(super) fn handle(
    client: &mut ClientStoreData,
    mod_id: &str,
    call: ClientCaptureCall,
) -> HostRet {
    let desk = client.presented.lock().capture.clone();
    match call {
        ClientCaptureCall::ClientWorldEventsBegin {
            scope,
            path,
            envelopes,
        } => {
            if let Err(bug) = checked_path("ClientWorldEventsBegin", &path).and_then(|()| {
                envelopes
                    .as_deref()
                    .map_or(Ok(()), |e| checked_path("ClientWorldEventsBegin", e))
            }) {
                return bug;
            }
            if let Some(refused) = write_refusal(client, scope) {
                return HostRet::refused(refused);
            }
            let Some(file) = files::locate(client, scope, &path) else {
                return HostRet::invalid(
                    "ClientWorldEventsBegin: scope World needs a world, and this instance runs \
                     on the shell with none"
                        .into(),
                );
            };
            let envelopes = envelopes
                .as_deref()
                .and_then(|e| files::locate(client, scope, e));
            let id = files::issue_id();
            let mut desk = desk.lock();
            let first_frame = desk.first_frame();
            if let Err(refused) =
                desk.logs
                    .begin(id, mod_id, file.clone(), envelopes.clone(), first_frame)
            {
                return HostRet::refused(refused);
            }
            client.files.touch(&file);
            if let Some(envelopes) = &envelopes {
                client.files.touch(envelopes);
            }
            HostRet::Ticket(id)
        }
        ClientCaptureCall::ClientWorldEventsEnd { events } => {
            let mut desk = desk.lock();
            if !desk.logs.owns(events, mod_id) {
                return HostRet::invalid(format!(
                    "ClientWorldEventsEnd: log {events} was never issued to this instance"
                ));
            }
            desk.logs.end(events, None);
            HostRet::Unit
        }
        ClientCaptureCall::ClientWorldEventsPoll { events } => {
            HostRet::ClientEvents(desk.lock().logs.poll(events, mod_id))
        }
        other => HostRet::invalid(format!(
            "non-events call {other:?} mis-routed to the events handler (host bug)"
        )),
    }
}
