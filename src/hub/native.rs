//! The hub's HTTP natively: a blocking read with ureq, on the thread that
//! asks for it (`background.rs`). The hub is plain HTTP on loopback
//! (VIZ_CONTRACT.md §2), so there is no TLS stack.

use std::time::Duration;

/// The hub gives a session ten seconds to answer and then returns `504`,
/// so a read that takes much longer than that has lost the hub.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// A hub that is running answers a connection at once on loopback.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// What every read goes through: short to connect, patient to read, and an
/// error status read as an answer so that its message can be shown.
pub struct Client(ureq::Agent);

pub fn client() -> Client {
    Client(
        ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(READ_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into(),
    )
}

/// The hub at `host:port`, as an address.
pub fn origin(hub: &str) -> String {
    format!("http://{hub}")
}

/// The address of `path` on the hub at `host:port`.
pub fn url(hub: &str, path: &str) -> String {
    format!("{}{path}", origin(hub))
}

/// `GET url?query`: the status and the body, read up to `limit` bytes, or
/// why there was no whole answer.
pub async fn get(
    client: &Client,
    url: &str,
    query: &[(&str, &str)],
    limit: u64,
) -> Result<(u16, String), String> {
    let mut response = client
        .0
        .get(url)
        .query_pairs(query.iter().copied())
        .call()
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    Ok((status, body))
}
