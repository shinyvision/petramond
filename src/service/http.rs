//! The transport: JSON round trips (the account's calls) and a streamed GET
//! with the timeouts a large download needs (the content library's).
//!
//! Nothing here decides what an answer MEANS beyond "the service answered"
//! and "it did not": the status and body go back to the caller, which maps
//! them onto its own errors. A refusal's BODY carries the reason a caller
//! branches on, so a 4xx arrives as an answer, never as a transport error.

use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::Value;

/// A JSON call's whole budget. A join waits on one, so it is short enough
/// that a dead service fails the join instead of hanging the connect screen.
pub const JSON_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of a JSON body read, success or refusal.
const JSON_MAX: u64 = 4 << 20;

/// Nothing answered, or not in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError {
    pub timed_out: bool,
    pub detail: String,
}

/// How a streamed call answered, as a caller other than the account sorts
/// it: only the content library's classification uses the `Busy` arm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceError {
    /// The stored sign-in is worthless; the caller clears it.
    SignInRequired(String),
    /// The service understood and refused.
    Refused(String),
    /// The service is rate-limiting this client: try again after the wait.
    Busy { retry_after: Duration },
    /// Nothing answered, or not usefully.
    Unreachable(String),
}

impl ServiceError {
    pub fn message(&self) -> String {
        match self {
            ServiceError::SignInRequired(m)
            | ServiceError::Refused(m)
            | ServiceError::Unreachable(m) => m.clone(),
            ServiceError::Busy { retry_after } => format!(
                "petramond.com is busy; trying again in {} s",
                retry_after.as_secs()
            ),
        }
    }
}

/// The budgets of a streamed GET.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    /// Until the response headers are in.
    pub headers: Duration,
    /// The longest gap between two bytes of the body.
    pub idle_read: Duration,
    /// The whole call, body included.
    pub total: Duration,
}

fn agent(config: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>) -> ureq::Agent {
    config
        .user_agent(crate::account::client_label())
        .http_status_as_error(false)
        .build()
        .into()
}

fn json_agent() -> ureq::Agent {
    agent(ureq::Agent::config_builder().timeout_global(Some(JSON_TIMEOUT)))
}

fn url(path: &str) -> String {
    format!("{}{path}", super::origin())
}

/// POST `body` as JSON to `path`; `bearer` adds the `Authorization` header.
/// Any HTTP answer is `Ok((status, body))`, a body that is not JSON reading
/// as `null`.
pub fn post_json(
    path: &str,
    body: &Value,
    bearer: Option<&str>,
) -> Result<(u16, Value), TransportError> {
    let mut request = json_agent()
        .post(&url(path))
        .header("Accept", "application/json");
    if let Some(token) = bearer {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    answered(request.send_json(body))
}

/// GET `path` as JSON; `bearer` adds the `Authorization` header.
pub fn get_json(path: &str, bearer: Option<&str>) -> Result<(u16, Value), TransportError> {
    let mut request = json_agent()
        .get(&url(path))
        .header("Accept", "application/json");
    if let Some(token) = bearer {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    answered(request.call())
}

fn answered(
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<(u16, Value), TransportError> {
    let mut response = result.map_err(transport)?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(JSON_MAX)
        .read_json()
        .unwrap_or(Value::Null);
    Ok((status, body))
}

fn transport(e: ureq::Error) -> TransportError {
    TransportError {
        timed_out: matches!(e, ureq::Error::Timeout(_)),
        detail: e.to_string(),
    }
}

/// A streamed answer: its status, the headers a download checks, and the
/// body as a reader that fails once no byte has arrived for
/// `Timeouts::idle_read`.
pub struct Stream {
    pub status: u16,
    pub content_length: Option<u64>,
    /// `X-Content-Sha256`, lowercased.
    pub sha256: Option<String>,
    /// `Retry-After`, as sent.
    pub retry_after: Option<String>,
    body: IdleBody,
}

impl Stream {
    /// A body read as JSON (a refusal's reason), bounded; `null` when it is
    /// not JSON.
    pub fn json(self) -> Value {
        let mut bytes = Vec::new();
        if self.take(JSON_MAX).read_to_end(&mut bytes).is_err() {
            return Value::Null;
        }
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    /// A stream over any reader: what a test hands the classifiers.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        status: u16,
        headers: &[(&str, &str)],
        body: impl Read + Send + 'static,
        idle_read: Duration,
    ) -> Self {
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| (*v).to_owned())
        };
        Self {
            status,
            content_length: header("Content-Length").and_then(|v| v.parse().ok()),
            sha256: header("X-Content-Sha256").map(|v| v.to_ascii_lowercase()),
            retry_after: header("Retry-After"),
            body: IdleBody::spawn(body, idle_read),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.body.read(out)
    }
}

/// GET `path`, streamed. Any HTTP answer is a [`Stream`]; only a transport
/// failure is an error, and it is always [`ServiceError::Unreachable`].
pub fn get_stream(
    path: &str,
    bearer: Option<&str>,
    timeouts: Timeouts,
) -> Result<Stream, ServiceError> {
    let agent = agent(
        ureq::Agent::config_builder()
            .timeout_global(Some(timeouts.total))
            .timeout_connect(Some(timeouts.connect))
            .timeout_recv_response(Some(timeouts.headers)),
    );
    let mut request = agent.get(&url(path));
    if let Some(token) = bearer {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let response = request.call().map_err(|e| {
        ServiceError::Unreachable(if matches!(e, ureq::Error::Timeout(_)) {
            "petramond.com did not respond".to_owned()
        } else {
            format!("Could not reach petramond.com: {e}")
        })
    })?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let status = response.status().as_u16();
    let content_length = header("Content-Length").and_then(|v| v.trim().parse().ok());
    let sha256 = header("X-Content-Sha256").map(|v| v.trim().to_ascii_lowercase());
    let retry_after = header("Retry-After");
    let reader = response.into_body().into_reader();
    Ok(Stream {
        status,
        content_length,
        sha256,
        retry_after,
        body: IdleBody::spawn(reader, timeouts.idle_read),
    })
}

/// A body read on its own thread, so a stalled connection fails the read
/// after `idle` instead of blocking it: the socket read itself cannot be
/// interrupted, but the caller no longer waits on it. The thread ends with
/// the body, or at the call's total budget.
struct IdleBody {
    rx: Receiver<io::Result<Vec<u8>>>,
    chunk: Vec<u8>,
    at: usize,
    idle: Duration,
    ended: bool,
}

const CHUNK: usize = 64 << 10;

impl IdleBody {
    fn spawn(mut body: impl Read + Send + 'static, idle: Duration) -> Self {
        let (tx, rx) = mpsc::sync_channel(4);
        let spawned = std::thread::Builder::new()
            .name("petramond-service-body".into())
            .spawn(move || loop {
                let mut chunk = vec![0; CHUNK];
                match body.read(&mut chunk) {
                    Ok(n) => {
                        chunk.truncate(n);
                        if tx.send(Ok(chunk)).is_err() || n == 0 {
                            return;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
            });
        let ended = spawned.is_err();
        Self {
            rx,
            chunk: Vec::new(),
            at: 0,
            idle,
            ended,
        }
    }
}

impl Read for IdleBody {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.at >= self.chunk.len() {
            if self.ended {
                return Ok(0);
            }
            match self.rx.recv_timeout(self.idle) {
                Ok(Ok(chunk)) if chunk.is_empty() => self.ended = true,
                Ok(Ok(chunk)) => {
                    self.chunk = chunk;
                    self.at = 0;
                }
                Ok(Err(e)) => return Err(e),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("petramond.com sent nothing for {} s", self.idle.as_secs()),
                    ))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the download stopped",
                    ))
                }
            }
        }
        let n = out.len().min(self.chunk.len() - self.at);
        out[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}
