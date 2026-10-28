use std::net::SocketAddr;
use std::time::Duration;

use http_body_util::{BodyExt, Limited};
use reqwest::{Method, StatusCode};

const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

// A launcher waits for the answer, so a hung agent has to come back as
// "could not be started" rather than leave it waiting.
const AGENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub addr: SocketAddr,
    pub token: String,
}

impl Agent {
    /// A fresh token for each boot, so one that leaks does not outlive it.
    pub fn new(addr: SocketAddr) -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("random bytes for the agent token");
        Self { addr, token: hex::encode(bytes) }
    }
}

/// One request to the agent, which speaks HTTP/1 on a loopback address passt
/// forwards into the bubble. Parameters go in a JSON body, never in the path.
pub async fn agent_request(
    agent: &Agent,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<(StatusCode, String), String> {
    let mut request = reqwest::Client::new()
        .request(method, format!("http://{}{}", agent.addr, path))
        .bearer_auth(&agent.token)
        // Covers reading the body too, not just the headers.
        .timeout(AGENT_TIMEOUT);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("the bubble did not answer: {e}"))?;

    let status = response.status();
    // The body comes from the guest, so its size is not ours to trust.
    let body = Limited::new(reqwest::Body::from(response), MAX_RESPONSE_BYTES)
        .collect()
        .await
        .map_err(|e| format!("could not read the answer: {e}"))?
        .to_bytes();
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}
