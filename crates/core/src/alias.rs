//! Public aliases: a name like `philipp-pc` that others can type instead of
//! the device ID. The server keeps them, bound to the device key like the ID.

use std::time::Duration;

use anyhow::{bail, Result};
use ctxremote_proto::framing;
use ctxremote_proto::rendezvous::{sign_alias_claim, ClientMsg, ServerMsg};
use ctxremote_proto::DeviceId;
use ed25519_dalek::SigningKey;
use tokio::time::timeout;

use crate::net;

pub use ctxremote_proto::rendezvous::normalize_alias;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// Servers from before aliases hang up on the request.
const OLD_SERVER: &str = "Der Server kennt noch keine Aliase (Server-Update nötig)";

/// Sets (or with `None` drops) this device's public alias. Returns it as the
/// server stored it (lowercase).
pub async fn claim(server: &str, key: &SigningKey, id: DeviceId, alias: Option<&str>) -> Result<Option<String>> {
    let (mut t, nonce) = net::dial(server).await?;
    let msg = ClientMsg::ClaimAlias {
        id,
        public_key: key.verifying_key().to_bytes(),
        signature: sign_alias_claim(key, &nonce, alias),
        alias: alias.map(str::to_string),
    };
    framing::send(&mut t, &msg).await?;
    match timeout(ANSWER_TIMEOUT, framing::recv::<ServerMsg>(&mut t)).await? {
        Ok(ServerMsg::AliasClaimed(result)) => Ok(result?),
        Ok(ServerMsg::Error(e)) => Err(e.into()),
        Ok(other) => bail!("unerwartete Serverantwort: {other:?}"),
        Err(_) => bail!(OLD_SERVER),
    }
}

/// Looks up the device behind a public alias.
pub async fn resolve(server: &str, alias: &str) -> Result<Option<DeviceId>> {
    let (mut t, _) = net::dial(server).await?;
    framing::send(&mut t, &ClientMsg::ResolveAlias { alias: alias.to_string() }).await?;
    match timeout(ANSWER_TIMEOUT, framing::recv::<ServerMsg>(&mut t)).await? {
        Ok(ServerMsg::AliasResolved(id)) => Ok(id),
        Ok(ServerMsg::Error(e)) => Err(e.into()),
        Ok(other) => bail!("unerwartete Serverantwort: {other:?}"),
        Err(_) => bail!(OLD_SERVER),
    }
}
