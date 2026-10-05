//! Length-prefixed message framing over any byte stream (TCP, or a QUIC
//! stream for direct connections through NAT).

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Large enough for an uncompressed 4K keyframe, small enough to bound memory.
pub const MAX_FRAME: usize = 32 * 1024 * 1024;

/// A byte stream a [`Transport`] can run over.
pub trait Io: AsyncRead + AsyncWrite + Send {}
impl<T: AsyncRead + AsyncWrite + Send> Io for T {}

pub type Conn = Pin<Box<dyn Io>>;
pub type Transport = Framed<Conn, LengthDelimitedCodec>;

pub fn transport(stream: TcpStream) -> Transport {
    let _ = stream.set_nodelay(true);
    transport_over(stream)
}

/// Frames any byte stream, e.g. a QUIC stream.
pub fn transport_over(stream: impl Io + 'static) -> Transport {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME)
        .new_codec();
    Framed::new(Box::pin(stream) as Conn, codec)
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

/// Sends `msg` with `trailer` in the same frame; older readers decode `msg`
/// and ignore the rest.
pub async fn send_with_trailer<T: Serialize, U: Serialize>(t: &mut Transport, msg: &T, trailer: &U) -> Result<()> {
    let mut bytes = postcard::to_stdvec(msg)?;
    bytes.extend(postcard::to_stdvec(trailer)?);
    t.send(Bytes::from(bytes)).await?;
    Ok(())
}

/// Like [`recv`], plus whatever followed the message in its frame.
pub async fn recv_with_rest<T: DeserializeOwned>(t: &mut Transport) -> Result<(T, Vec<u8>)> {
    let frame = t
        .next()
        .await
        .ok_or_else(|| anyhow!("Verbindung wurde geschlossen"))??;
    let (msg, rest) = postcard::take_from_bytes::<T>(&frame).context("ungültige Nachricht")?;
    Ok((msg, rest.to_vec()))
}

pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T> {
    postcard::from_bytes(frame).context("ungültige Nachricht")
}
