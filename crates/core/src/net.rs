//! Connections to the rendezvous server.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use ctxremote_proto::framing::{self, Transport};
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
        ServerMsg::Challenge { nonce, .. } => Ok((t, nonce)),
        other => bail!("unerwartete Serverantwort: {other:?}"),
    }
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
