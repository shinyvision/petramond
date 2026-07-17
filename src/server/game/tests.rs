use crate::net::protocol::ServerToClient;
use crate::server::chat::ChatTargets;

fn chat_texts(msgs: &[ServerToClient]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerToClient::ChatLine(line) => Some(
                line.spans
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn targeted_chat_reaches_only_listed_sessions() {
    let (mut server, _) = crate::game::session::build_session("", 1, 2);
    let player = crate::game::session::spawn_player(server.world.seed);
    let remote_s = server.add_session_for_test(player);
    let remote_id = server.sessions[remote_s].id;

    server.enqueue_authored_chat("only-remote", ChatTargets::Players(vec![remote_id]));
    server.enqueue_authored_chat("everyone", ChatTargets::All);

    let out = server.pump(0.0, &mut Vec::new());
    let local = chat_texts(&out.msgs);
    assert!(
        !local.iter().any(|t| t.contains("only-remote")),
        "local must not receive a remote-only line"
    );
    assert!(
        local.iter().any(|t| t.contains("everyone")),
        "local must receive broadcast"
    );

    let remote_msgs = out
        .remote
        .iter()
        .find(|(id, _)| *id == remote_id)
        .map(|(_, msgs)| msgs.as_slice())
        .unwrap_or(&[]);
    let remote = chat_texts(remote_msgs);
    assert!(
        remote.iter().any(|t| t.contains("only-remote")),
        "remote must receive its targeted line"
    );
    assert!(
        remote.iter().any(|t| t.contains("everyone")),
        "remote must receive broadcast"
    );
}
