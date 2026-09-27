use super::*;
use serde_json::json;

/// The status→error mapping every caller branches on. `signInRequired` is the
/// only thing that may discard a player's stored credential, and a 5xx or a
/// malformed body must never read that way: a service outage would sign
/// everybody out.
#[test]
fn only_a_sign_in_required_refusal_clears_the_stored_credential() {
    let dead = http::classify(
        401,
        json!({"code": "invalid_token", "message": "gone", "signInRequired": true}),
    )
    .expect_err("401 is a refusal");
    assert!(dead.clears_sign_in());

    for (status, body) in [
        (
            401,
            json!({"code": "invalid_credentials", "message": "nope"}),
        ),
        (
            403,
            json!({"code": "email_unverified", "message": "verify"}),
        ),
        (429, json!({"code": "too_many_attempts", "message": "wait"})),
        (500, json!(null)),
        (502, json!({"message": "bad gateway"})),
    ] {
        let e = http::classify(status, body).expect_err("a refusal");
        assert!(
            !e.clears_sign_in(),
            "status {status} must not sign the player out"
        );
    }
}

#[test]
fn a_reply_missing_a_field_is_unreachable_not_a_successful_sign_in() {
    let body = json!({"userId": 1, "username": "explorer", "avatarUrl": "/a.png"});
    let e = http::field_str(&body, "profileUrl").expect_err("missing field");
    assert!(matches!(e, AccountError::Unreachable(_)));
    assert!(!e.clears_sign_in());
}

/// Lifetimes arrive as durations and are banked against the local clock, so the
/// refresh decision never depends on the service and the client agreeing on the
/// time of day.
#[test]
fn durations_from_the_service_become_local_deadlines() {
    let signed_in = SignedIn {
        token: Some("fresh".to_owned()),
        identity: AccountIdentity {
            user_id: 4,
            username: "explorer".to_owned(),
            avatar_url: "/a.png".to_owned(),
            profile_url: "/players/explorer".to_owned(),
        },
        refresh_in: 60,
        expires_in: 600,
    };
    let saved = SavedSignIn::from_service("previous", &signed_in);
    assert_eq!(saved.token, "fresh");
    assert!(!saved.due_for_refresh() && !saved.expired());

    let due = SavedSignIn::from_service(
        "previous",
        &SignedIn {
            refresh_in: -1,
            ..signed_in.clone()
        },
    );
    assert!(due.due_for_refresh(), "a passed refresh deadline is due");
    assert!(!due.expired(), "due for rotation is not expired");

    // An answer that mints no token (an identity check) keeps the one in hand.
    let kept = SavedSignIn::from_service(
        "previous",
        &SignedIn {
            token: None,
            ..signed_in
        },
    );
    assert_eq!(kept.token, "previous");
}

#[test]
fn offline_mode_is_opt_in_through_one_spelling_set() {
    // The parse, not the process environment: the env var itself is global
    // state no test may set for the rest of the suite.
    assert!(AccountPolicy::default().requires_account());
    assert!(!AccountPolicy::Offline.requires_account());
}

#[test]
fn a_server_id_is_unguessable_and_fresh_per_server() {
    let a = new_server_id();
    let b = new_server_id();
    assert_eq!(a.len(), 32, "128 bits of hex");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

#[test]
fn the_service_url_trims_to_a_prefix_paths_append_to() {
    // The default; the env override is process-global and left to deployment.
    let url = service_url();
    assert!(!url.ends_with('/'), "{url} must not double the path slash");
}

/// The cross-repository check: a real sign-in against a real account service,
/// and a real join through an ONLINE server that redeems the ticket.
///
/// OPT-IN, like the website's own PostgreSQL tests: it runs only when
/// `PETRAMOND_ACCOUNT_URL`, `PETRAMOND_TEST_ACCOUNT` and
/// `PETRAMOND_TEST_PASSWORD` are all set, and it WRITES `account.json` in the
/// data dir — point `PETRAMOND_DATA_DIR` at a scratch directory. The ordinary
/// suite must never reach the network, so nothing here is reachable from
/// `make test`.
///
/// ```text
/// PETRAMOND_DATA_DIR=/tmp/scratch PETRAMOND_ACCOUNT_URL=http://localhost:8044 \
///   PETRAMOND_TEST_ACCOUNT=someone PETRAMOND_TEST_PASSWORD=... \
///   cargo test --profile fasttest -p petramond --lib account::tests::live_ -- --ignored
/// ```
#[test]
#[ignore = "needs a running account service and test credentials"]
fn live_sign_in_mints_a_ticket_an_online_server_redeems() {
    let (Ok(identifier), Ok(password)) = (
        std::env::var("PETRAMOND_TEST_ACCOUNT"),
        std::env::var("PETRAMOND_TEST_PASSWORD"),
    ) else {
        panic!("set PETRAMOND_TEST_ACCOUNT and PETRAMOND_TEST_PASSWORD");
    };

    let identity = session::sign_in(&identifier, &password).expect("sign in");
    assert!(!identity.username.is_empty());
    let saved = session::current().expect("the sign-in is stored");
    assert!(!saved.token.is_empty() && !saved.due_for_refresh() && !saved.expired());
    // A bearer credential on disk must not be readable by other users.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = crate::save::base_data_dir().join("account.json");
        let mode = std::fs::metadata(&file)
            .expect("account.json exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "{} is {mode:o}, not owner-only",
            file.display()
        );
    }

    // A ticket is good exactly once, and only at the server it names.
    let server_id = new_server_id();
    let ticket = session::join_ticket_for(&server_id).expect("mint a ticket");
    assert_eq!(
        verify_join(&ticket, &server_id).expect("redeem"),
        identity,
        "the server learns the account behind the ticket"
    );
    assert!(
        verify_join(&ticket, &server_id).is_err(),
        "a redeemed ticket cannot be replayed"
    );
    let elsewhere = session::join_ticket_for(&server_id).expect("mint a ticket");
    assert!(
        verify_join(&elsewhere, &new_server_id()).is_err(),
        "a ticket is worthless at another server"
    );

    // The whole join, through the real handshake against a real ONLINE server.
    let mut server = crate::server::session_build::build_headless_session("", 21, 4);
    server.set_account_policy(AccountPolicy::Online);
    let mut host = crate::server::handle::spawn(server);
    host.unthrottle_for_test();
    let port = host.open_to_lan(0).expect("bind an ephemeral port");
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .expect("read timeout");
    let identity = crate::net::identity::PlayerIdentity::generate().expect("os randomness");
    let join = crate::net::handshake::client_handshake(
        &mut stream,
        &identity,
        |offer| {
            assert!(
                offer.requires_account,
                "an online server demands an account"
            );
            session::join_ticket_for(offer.server_id)
                .map(crate::net::protocol::JoinCredential::Ticket)
                .map_err(|e| crate::net::handshake::HandshakeError::Credential {
                    sign_in_required: e.clears_sign_in(),
                    message: e.message().to_owned(),
                })
        },
        4,
        &crate::net::handshake::installed_mod_ids(),
        Vec::new(),
    )
    .expect("the verified join is accepted");
    assert_eq!(
        join.join.players.len(),
        0,
        "a headless server has no other session yet"
    );
    host.shutdown_and_join();

    session::sign_out();
    assert!(session::current().is_none(), "sign-out clears the store");
}
