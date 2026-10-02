//! Length-prefixed message framing over TCP.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Large enough for an uncompressed 4K keyframe, small enough to bound memory.
pub const MAX_FRAME: usize = 32 * 1024 * 1024;

pub type Transport = Framed<TcpStream, LengthDelimitedCodec>;

pub fn transport(stream: TcpStream) -> Transport {
    let _ = stream.set_nodelay(true);
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME)
        .new_codec();
    Framed::new(stream, codec)
}

pub async fn send<T: Serialize>(t: &mut Transport, msg: &T) -> Result<()> {
    let bytes = postcard::to_stdvec(msg)?;
    t.send(Bytes::from(bytes)).await?;
    Ok(())
}

pub async fn recv<T: DeserializeOwned>(t: &mut Transport) -> Result<T> {
    let frame = t
        .next()
        .await
        .ok_or_else(|| anyhow!("Verbindung wurde geschlossen"))??;
    decode(&frame)
}

pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T> {
    postcard::from_bytes(frame).context("ungültige Nachricht")
}
