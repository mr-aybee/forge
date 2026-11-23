//! Frames of a container shell stream (D-114).
//!
//! The same codec runs between the root helper and the agent (Unix socket)
//! and between the agent and the control plane (HTTP/2 stream), so the agent
//! relays bytes without parsing them. A frame is
//! `[kind u8][length u32 big-endian][payload]`, the payload at most
//! [`MAX_PAYLOAD`] bytes.
//!
//! | Kind | Value | Payload | Direction |
//! | --- | --- | --- | --- |
//! | `Data` | 1 | terminal bytes | both |
//! | `Resize` | 2 | cols `u16`, rows `u16` (big-endian) | to the node |
//! | `Eof` | 3 | empty | both |
//! | `Exit` | 4 | exit code `i32` (big-endian) | from the node |
//! | `Error` | 5 | UTF-8 message | from the node |
//! | `Ready` | 6 | empty | from the node, first frame |

/// Largest payload of one frame.
pub const MAX_PAYLOAD: usize = 64 * 1024;
const HEADER: usize = 5;

/// One frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Data(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Eof,
    Exit(i32),
    Error(String),
    /// The shell runs: the first frame the node sends on success.
    Ready,
}

/// A malformed stream: the reader stops.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("unknown frame kind {0}")]
    UnknownKind(u8),
    #[error("frame of {0} bytes is larger than {MAX_PAYLOAD}")]
    TooLarge(usize),
    #[error("malformed {0} frame")]
    Malformed(&'static str),
}

impl Frame {
    /// The frame on the wire. Data and error payloads longer than
    /// [`MAX_PAYLOAD`] are cut; use [`encode_data`] for long data.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload): (u8, Vec<u8>) = match self {
            Frame::Data(d) => (1, d[..d.len().min(MAX_PAYLOAD)].to_vec()),
            Frame::Resize { cols, rows } => {
                let mut p = cols.to_be_bytes().to_vec();
                p.extend_from_slice(&rows.to_be_bytes());
                (2, p)
            }
            Frame::Eof => (3, Vec::new()),
            Frame::Exit(code) => (4, code.to_be_bytes().to_vec()),
            Frame::Error(m) => {
                let mut end = m.len().min(MAX_PAYLOAD);
                while !m.is_char_boundary(end) {
                    end -= 1;
                }
                (5, m.as_bytes()[..end].to_vec())
            }
            Frame::Ready => (6, Vec::new()),
        };
        let mut out = Vec::with_capacity(HEADER + payload.len());
        out.push(kind);
        // At most MAX_PAYLOAD, so it fits.
        out.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(u32::MAX).to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }
}

/// `data` as one or more `Data` frames.
#[must_use]
pub fn encode_data(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + HEADER * (data.len() / MAX_PAYLOAD + 1));
    for chunk in data.chunks(MAX_PAYLOAD) {
        out.extend_from_slice(&Frame::Data(chunk.to_vec()).encode());
    }
    out
}

/// Turns a byte stream, in pieces of any size, back into frames.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
    /// Start of the unread bytes: frames advance it; consumed bytes are
    /// dropped once per `push`, not once per frame.
    pos: usize,
}

impl Decoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// Whether no partial frame is buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pos == self.buf.len()
    }

    /// The next whole frame, `None` until one is buffered.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        let rest = &self.buf[self.pos..];
        if rest.len() < HEADER {
            return Ok(None);
        }
        let kind = rest[0];
        if !(1..=6).contains(&kind) {
            return Err(FrameError::UnknownKind(kind));
        }
        let len = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        if len > MAX_PAYLOAD {
            return Err(FrameError::TooLarge(len));
        }
        if rest.len() < HEADER + len {
            return Ok(None);
        }
        let payload = &rest[HEADER..HEADER + len];
        let frame = match kind {
            1 => Frame::Data(payload.to_vec()),
            2 => match payload {
                [c0, c1, r0, r1] => {
                    Frame::Resize { cols: u16::from_be_bytes([*c0, *c1]), rows: u16::from_be_bytes([*r0, *r1]) }
                }
                _ => return Err(FrameError::Malformed("resize")),
            },
            3 if payload.is_empty() => Frame::Eof,
            4 => match payload {
                [a, b, c, d] => Frame::Exit(i32::from_be_bytes([*a, *b, *c, *d])),
                _ => return Err(FrameError::Malformed("exit")),
            },
            5 => Frame::Error(String::from_utf8_lossy(payload).into_owned()),
            6 if payload.is_empty() => Frame::Ready,
            3 => return Err(FrameError::Malformed("eof")),
            _ => return Err(FrameError::Malformed("ready")),
        };
        self.pos += HEADER + len;
        Ok(Some(frame))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_in_pieces_of_any_size() {
        let frames = vec![
            Frame::Ready,
            Frame::Data(b"ls -la\r".to_vec()),
            Frame::Resize { cols: 200, rows: 50 },
            Frame::Data(Vec::new()),
            Frame::Error("the container is not running".into()),
            Frame::Eof,
            Frame::Exit(-3),
            Frame::Exit(127),
        ];
        let wire: Vec<u8> = frames.iter().flat_map(Frame::encode).collect();
        for step in [1, 2, 3, 7, wire.len()] {
            let mut d = Decoder::new();
            let mut got = Vec::new();
            for piece in wire.chunks(step) {
                d.push(piece);
                while let Some(f) = d.next_frame().unwrap() {
                    got.push(f);
                }
            }
            assert_eq!(got, frames, "pieces of {step}");
            assert!(d.is_empty());
        }
    }

    #[test]
    fn long_data_is_split_and_oversized_or_unknown_frames_are_refused() {
        let big = vec![7u8; MAX_PAYLOAD * 2 + 10];
        let mut d = Decoder::new();
        d.push(&encode_data(&big));
        let mut back = Vec::new();
        while let Some(Frame::Data(x)) = d.next_frame().unwrap() {
            assert!(x.len() <= MAX_PAYLOAD);
            back.extend_from_slice(&x);
        }
        assert_eq!(back, big);

        let mut d = Decoder::new();
        let mut header = vec![1u8];
        header.extend_from_slice(&u32::try_from(MAX_PAYLOAD + 1).unwrap().to_be_bytes());
        d.push(&header);
        assert_eq!(d.next_frame(), Err(FrameError::TooLarge(MAX_PAYLOAD + 1)));
        let mut d = Decoder::new();
        d.push(&[9, 0, 0, 0, 0]);
        assert_eq!(d.next_frame(), Err(FrameError::UnknownKind(9)));
        let mut d = Decoder::new();
        d.push(&[2, 0, 0, 0, 1, 0]);
        assert_eq!(d.next_frame(), Err(FrameError::Malformed("resize")));
        // A cut error message stays valid UTF-8.
        let long = "é".repeat(MAX_PAYLOAD);
        let mut d = Decoder::new();
        d.push(&Frame::Error(long).encode());
        assert!(matches!(d.next_frame().unwrap(), Some(Frame::Error(m)) if m.len() <= MAX_PAYLOAD));
    }
}
