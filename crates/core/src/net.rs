//! Connections to the rendezvous server.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport};
use ctxremote_proto::tunnel;
use ctxremote_proto::rendezvous::{Nonce, ServerMsg};
use ctxremote_proto::PROTOCOL_VERSION;
use tokio::net::TcpStream;
use tokio::time::timeout;

const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens a connection and reads the server's challenge.
pub async fn dial(addr: &str) -> Result<(Transport, Nonce)> {
    let stream = timeout(DIAL_TIMEOUT, TcpStream::connect(addr))
        .await
        .with_context(|| format!("Server {addr} antwortet nicht"))?
        .with_context(|| format!("Server {addr} nicht erreichbar"))?;
    let mut t = framing::transport(stream);
    match timeout(DIAL_TIMEOUT, framing::recv::<ServerMsg>(&mut t)).await?? {
        ServerMsg::Challenge { version, .. } if version != PROTOCOL_VERSION => {
            bail!("Server verwendet Protokoll {version}, erwartet {PROTOCOL_VERSION}")
        }
        ServerMsg::Challenge { nonce, .. } => match server_key() {
            // Encrypted, and only with the genuine server (see `proto::tunnel`).
            Some(key) => timeout(DIAL_TIMEOUT, tunnel::client(t, &key, nonce)).await.context("Server antwortet nicht")?,
            None => Ok((t, nonce)),
        },
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}

/// Built into release builds (`CTXREMOTE_SERVER_KEY`, see `docs/DEPLOY.md`).
const BUILT_IN_KEY: Option<&str> = option_env!("CTXREMOTE_SERVER_KEY");

/// The server's public tunnel key: `CTXREMOTE_SERVER_KEY` at run time (tests,
/// another server), else the built-in one. Without a key, connections stay
/// unencrypted, which only development builds do.
pub fn server_key() -> Option<[u8; 32]> {
    let text = std::env::var("CTXREMOTE_SERVER_KEY")
        .ok()
        .or_else(|| BUILT_IN_KEY.map(str::to_string))
        // An unset repository variable builds in an empty string.
        .filter(|k| !k.trim().is_empty())?;
    let key = hex::decode(text.trim()).ok().and_then(|k| k.try_into().ok());
    if key.is_none() {
        tracing::warn!("CTXREMOTE_SERVER_KEY ist kein gültiger Schlüssel");
    }
    key
}

/// Waits for the server to pair this connection with its peer.
pub async fn await_ready(t: &mut Transport, wait: Duration) -> Result<()> {
    match timeout(wait, framing::recv::<ServerMsg>(t))
        .await
        .context("Zeitüberschreitung beim Verbindungsaufbau")??
    {
        ServerMsg::Ready => Ok(()),
        ServerMsg::Error(e) => Err(e.into()),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
}
