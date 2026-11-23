//! Framing: `u32 BE header_len | u32 BE blob_len | header | blob`.
//!
//! The header is one JSON message; the blob carries opaque bytes (a CLEF
//! body, encoded heartbeat telemetry) so neither side base64-encodes or
//! decodes them in between. Both lengths are checked against a cap before
//! anything is allocated.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MIB: u32 = 1024 * 1024;

/// Largest header and blob a reader accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub header: u32,
    pub blob: u32,
}

/// What the worker accepts from forge-server: map pushes are the largest
/// headers; a blob is at most one HTTP request body.
pub const REQUEST_LIMITS: Limits = Limits { header: 16 * MIB, blob: 16 * MIB };
/// What forge-server accepts from the worker: a page of log events.
pub const RESPONSE_LIMITS: Limits = Limits { header: 32 * MIB, blob: 0 };

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The peer closed the connection between frames.
    #[error("the connection was closed")]
    Closed,
    #[error("{what} of {len} bytes exceeds the limit of {max}")]
    TooLarge { what: &'static str, len: u32, max: u32 },
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed message: {0}")]
    Json(#[from] serde_json::Error),
}

/// Writes one frame and flushes it.
pub async fn write_message<W, T>(w: &mut W, msg: &T, blob: &[u8]) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let header = serde_json::to_vec(msg)?;
    let too_large = |what| FrameError::TooLarge { what, len: u32::MAX, max: u32::MAX };
    let hlen = u32::try_from(header.len()).map_err(|_| too_large("header"))?;
    let blen = u32::try_from(blob.len()).map_err(|_| too_large("blob"))?;
    let mut prefix = [0u8; 8];
    prefix[..4].copy_from_slice(&hlen.to_be_bytes());
    prefix[4..].copy_from_slice(&blen.to_be_bytes());
    w.write_all(&prefix).await?;
    w.write_all(&header).await?;
    if !blob.is_empty() {
        w.write_all(blob).await?;
    }
    w.flush().await?;
    Ok(())
}

/// Reads one frame: the decoded header and the blob.
pub async fn read_message<R, T>(r: &mut R, limits: Limits) -> Result<(T, Vec<u8>), FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut prefix = [0u8; 8];
    match r.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let hlen = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
    let blen = u32::from_be_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
    if hlen > limits.header {
        return Err(FrameError::TooLarge { what: "header", len: hlen, max: limits.header });
    }
    if blen > limits.blob {
        return Err(FrameError::TooLarge { what: "blob", len: blen, max: limits.blob });
    }
    let mut header = vec![0u8; hlen as usize];
    r.read_exact(&mut header).await?;
    let mut blob = vec![0u8; blen as usize];
    if blen > 0 {
        r.read_exact(&mut blob).await?;
    }
    Ok((serde_json::from_slice(&header)?, blob))
}
