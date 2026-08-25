use std::os::fd::OwnedFd;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::client::conn::http2::SendRequest;
use hyper::header::CONTENT_TYPE;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;

const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

const READY_LINE: &[u8] = b"BUBBLES-AGENT-READY\n";

/// The HTTP/2 connection to a running bubble's agent, over its virtio-console port.
#[derive(Clone, Debug)]
pub struct Agent(SendRequest<Full<Bytes>>);

impl Agent {
    /// Waits for the agent's ready line, then connects. Fails once the VM is
    /// gone, as that closes the port.
    pub async fn connect(console_port: OwnedFd) -> Result<Self, String> {
        relm4::spawn(async move {
            let mut stream = Ok(std::os::unix::net::UnixStream::from(console_port))
                .and_then(|s| s.set_nonblocking(true).map(|()| s))
                .and_then(UnixStream::from_std)
                .map_err(|e| format!("the agent port is unusable: {e}"))?;
            wait_for_ready_line(&mut stream).await?;

            let (sender, connection) =
                hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
                    .await
                    .map_err(|e| format!("the agent refused the connection: {e}"))?;
            tokio::spawn(async move {
                if let Err(error) = connection.await {
                    eprintln!("agent connection ended: {error}");
                }
            });
            Ok(Agent(sender))
        })
        .await
        .map_err(|e| format!("connecting to the agent failed: {e}"))?
    }
}

// Read a byte at a time: whatever follows belongs to HTTP/2.
async fn wait_for_ready_line(stream: &mut UnixStream) -> Result<(), String> {
    let mut line = Vec::with_capacity(READY_LINE.len());
    while line.len() < READY_LINE.len() {
        let byte = stream
            .read_u8()
            .await
            .map_err(|e| format!("the agent port closed before the agent was ready: {e}"))?;
        line.push(byte);
    }
    if line != READY_LINE {
        return Err("the agent sent an unexpected greeting".to_string());
    }
    Ok(())
}

/// One request to the agent.
pub async fn agent_request(
    agent: &Agent,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<(StatusCode, String), String> {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("http://0.0.0.0{path}"));
    let body = match body {
        Some(body) => {
            request = request.header(CONTENT_TYPE, "application/json");
            Full::new(Bytes::from(body.to_string()))
        }
        None => Full::new(Bytes::new()),
    };
    let request = request
        .body(body)
        .map_err(|e| format!("could not build the request: {e}"))?;
    let response = agent
        .0
        .clone()
        .send_request(request)
        .await
        .map_err(|e| format!("the bubble did not answer: {e}"))?;

    let status = response.status();
    // The body comes from the guest, so its size is not ours to trust.
    let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
        .collect()
        .await
        .map_err(|e| format!("could not read the answer: {e}"))?
        .to_bytes();
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}
