use mod_api::{ClientFileAnswer, ClientFileCall, ClientStorageScope, HostRet};

use crate::modding::client::files::{self, FileRef, TicketSink};
use crate::modding::client::state::ClientStoreData;

const NO_WORLD_BUCKET: &str = "there is no world bucket on the shell; use scope Pack";
const NO_FOLDER: &str = "no folder has been chosen for this yet";

pub(super) fn checked_path(call: &str, path: &str) -> Result<(), HostRet> {
    match mod_api::file_path_problem(path) {
        Some(problem) => Err(HostRet::invalid(format!(
            "{call}: path '{path}': {problem}"
        ))),
        None => Ok(()),
    }
}

pub(super) fn presents(client: &ClientStoreData) -> bool {
    !client.shell
        && matches!(
            client.presented.lock().context,
            mod_api::ClientContext::Presentation { .. }
        )
}

pub(super) fn write_refusal(client: &ClientStoreData, scope: ClientStorageScope) -> Option<String> {
    match scope {
        ClientStorageScope::World if presents(client) => {
            Some("a presentation's world bucket is not this session's to keep".into())
        }
        _ => read_refusal(client, scope),
    }
}

fn read_refusal(client: &ClientStoreData, scope: ClientStorageScope) -> Option<String> {
    match scope {
        ClientStorageScope::Pack => None,
        ClientStorageScope::World => client.shell.then(|| NO_WORLD_BUCKET.into()),
        ClientStorageScope::Chosen(_) => files::root(client, scope)
            .is_none()
            .then(|| NO_FOLDER.into()),
    }
}

fn reply_fits(call: &str, what: &str, bytes: u64, guest_memory_max: u64) -> Result<(), HostRet> {
    if bytes > guest_memory_max {
        return Err(HostRet::invalid(format!(
            "{call}: a {what} of {bytes} bytes exceeds the {guest_memory_max} bytes this \
             instance can address"
        )));
    }
    Ok(())
}

fn bucket_file(
    client: &ClientStoreData,
    scope: ClientStorageScope,
    path: &str,
) -> Result<FileRef, HostRet> {
    files::locate(client, scope, path).ok_or_else(|| {
        HostRet::invalid(
            "client file scope World needs a world, and this instance runs on the shell with \
             none; use scope Pack"
                .into(),
        )
    })
}

fn ticketed(
    client: &mut ClientStoreData,
    scope: ClientStorageScope,
    writes: bool,
    paths: &[&str],
    start: impl FnOnce(&[FileRef], TicketSink) -> Result<(), String>,
) -> HostRet {
    let refusal = if writes {
        write_refusal(client, scope)
    } else {
        read_refusal(client, scope)
    };
    if let Some(why) = refusal {
        return HostRet::refused(why);
    }
    let Some(named): Option<Vec<FileRef>> = paths
        .iter()
        .map(|path| files::locate(client, scope, path))
        .collect()
    else {
        return HostRet::refused(NO_WORLD_BUCKET);
    };
    if writes {
        for file in &named {
            client.files.touch(file);
        }
    }
    let sink = client.files.issue();
    let ticket = sink.ticket();
    match start(&named, sink) {
        Ok(()) => HostRet::Ticket(ticket),
        Err(why) => {
            client.files.withdraw(ticket);
            HostRet::refused(why)
        }
    }
}

fn landed(sink: TicketSink) -> impl FnOnce(Result<[u64; 2], String>) + Send + 'static {
    move |result| {
        sink.finish(result.map(|range| ClientFileAnswer::Done {
            range: Some(range),
            envelope: None,
        }))
    }
}

fn finished(sink: TicketSink) -> impl FnOnce(Result<(), String>) + Send + 'static {
    move |result| {
        sink.finish(result.map(|()| ClientFileAnswer::Done {
            range: None,
            envelope: None,
        }))
    }
}

pub(super) fn handle(
    client: &mut ClientStoreData,
    guest_memory_max: u64,
    call: ClientFileCall,
) -> HostRet {
    let checked = match &call {
        ClientFileCall::ClientFileAppend { path, .. } => checked_path("ClientFileAppend", path),
        ClientFileCall::ClientFileWrite { path, .. } => checked_path("ClientFileWrite", path),
        ClientFileCall::ClientFileSync { path, .. } => checked_path("ClientFileSync", path),
        ClientFileCall::ClientFileRename { from, to, .. } => checked_path("ClientFileRename", from)
            .and_then(|()| checked_path("ClientFileRename", to)),
        ClientFileCall::ClientFileDelete { path, .. } => checked_path("ClientFileDelete", path),
        ClientFileCall::ClientFileRead { path, len, .. } => checked_path("ClientFileRead", path)
            .and_then(|()| reply_fits("ClientFileRead", "read", *len, guest_memory_max)),
        ClientFileCall::ClientFileList { dir, max_bytes, .. } => (if dir.is_empty() {
            Ok(())
        } else {
            checked_path("ClientFileList", dir)
        })
        .and_then(|()| reply_fits("ClientFileList", "listing", *max_bytes, guest_memory_max)),
        ClientFileCall::ClientFileStat { path, .. } => checked_path("ClientFileStat", path),
        ClientFileCall::ClientFileReveal { path, .. } => checked_path("ClientFileReveal", path),
        _ => Ok(()),
    };
    if let Err(bug) = checked {
        return bug;
    }
    match call {
        ClientFileCall::ClientFileAppend { scope, path, bytes } => {
            ticketed(client, scope, true, &[&path], |named, sink| {
                files::append(&named[0], bytes, landed(sink))
            })
        }
        ClientFileCall::ClientFileWrite {
            scope,
            path,
            offset,
            bytes,
            truncate,
        } => ticketed(client, scope, true, &[&path], |named, sink| {
            files::write(&named[0], offset, bytes, truncate, landed(sink))
        }),
        ClientFileCall::ClientFileSync { scope, path } => {
            ticketed(client, scope, true, &[&path], |named, sink| {
                files::sync(&named[0], finished(sink));
                Ok(())
            })
        }
        ClientFileCall::ClientFileRename { scope, from, to } => {
            ticketed(client, scope, true, &[&from, &to], |named, sink| {
                files::rename(&named[0], &named[1], finished(sink))
            })
        }
        ClientFileCall::ClientFileDelete { scope, path } => {
            ticketed(client, scope, true, &[&path], |named, sink| {
                files::delete(&named[0], finished(sink))
            })
        }
        ClientFileCall::ClientFileRead {
            scope,
            path,
            offset,
            len,
        } => ticketed(client, scope, false, &[&path], |named, sink| {
            files::read(&named[0], offset, len, move |result| {
                sink.finish(result.map(ClientFileAnswer::Read))
            });
            Ok(())
        }),
        ClientFileCall::ClientFileList {
            scope,
            dir,
            after,
            max_bytes,
        } => ticketed(client, scope, false, &[&dir], |named, sink| {
            files::list(&named[0], after, max_bytes, move |result| {
                sink.finish(
                    result.map(|(entries, more)| ClientFileAnswer::Listing { entries, more }),
                )
            });
            Ok(())
        }),
        ClientFileCall::ClientFilePoll { ticket } => match client.files.poll(ticket) {
            Ok(None) => HostRet::ClientFilePolled(None),
            Ok(Some(Ok(answer))) => HostRet::ClientFilePolled(Some(answer)),
            Ok(Some(Err(failed))) => HostRet::refused(failed),
            Err(why) => HostRet::invalid(format!("ClientFilePoll: {why}")),
        },
        ClientFileCall::ClientFileStat {
            scope: scope @ ClientStorageScope::Chosen(_),
            path,
        } => HostRet::ClientFileStat(
            files::locate(client, scope, &path).and_then(|file| files::stat(&file)),
        ),
        ClientFileCall::ClientFileReveal {
            scope: scope @ ClientStorageScope::Chosen(_),
            path,
        } => HostRet::Bool(files::locate(client, scope, &path).is_some_and(|f| files::reveal(&f))),
        ClientFileCall::ClientFileStat { scope, path } => match bucket_file(client, scope, &path) {
            Ok(file) => HostRet::ClientFileStat(files::stat(&file)),
            Err(bug) => bug,
        },
        ClientFileCall::ClientFileReveal { scope, path } => match bucket_file(client, scope, &path)
        {
            Ok(file) => HostRet::Bool(files::reveal(&file)),
            Err(bug) => bug,
        },
        ClientFileCall::ClientFolderChoose { folder, title } => {
            let pack = client.pack_storage.dir().to_path_buf();
            let sink = client.files.issue();
            let ticket = sink.ticket();
            match files::folders::choose(&pack, folder, title, sink) {
                Ok(()) => HostRet::Ticket(ticket),
                Err(why) => {
                    client.files.withdraw(ticket);
                    HostRet::refused(why)
                }
            }
        }
        ClientFileCall::ClientFolderState { folder } => {
            HostRet::ClientFolder(files::folders::info(client.pack_storage.dir(), folder))
        }
    }
}
