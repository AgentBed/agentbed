//! Length-prefixed framing: `[u32 big-endian length][length bytes UTF-8 JSON]`.
//!
//! Rules, all of which exist because a framing ambiguity is a smuggling
//! primitive:
//!
//! - The declared length is compared against [`MAX_FRAME_BYTES`] **before any
//!   allocation**, so a hostile 4-byte prefix cannot make the reader reserve
//!   4 GiB.
//! - A body is read with `read_exact`. A short read is [`FrameError::Truncated`]
//!   and the partial bytes are dropped without being parsed: a partial frame is
//!   never processed.
//! - Nothing is ever skipped to "resynchronize". If the reader cannot know
//!   where the next frame begins, it says so via
//!   [`FrameError::stream_position_lost`] and the caller closes the connection.

use std::io::{self, Read, Write};

/// Hard cap on a single frame body. Gate 0 carries one read operation with no
/// payload; 64 KiB is already generous and bounds per-connection memory.
pub const MAX_FRAME_BYTES: u32 = 64 * 1024;

/// Length prefix width in bytes.
pub const LENGTH_PREFIX_BYTES: usize = 4;

/// Why a frame could not be read or written.
#[derive(Debug)]
pub enum FrameError {
    /// Clean end of stream **on a frame boundary**: the peer finished. Not an
    /// error condition in itself.
    Eof,
    /// End of stream in the middle of a length prefix or a body. The partial
    /// bytes are discarded unparsed.
    Truncated {
        /// Bytes the prefix promised.
        expected: usize,
        /// Bytes actually available before EOF.
        received: usize,
    },
    /// Declared length exceeds [`MAX_FRAME_BYTES`]. Rejected before allocating.
    Oversize {
        /// The length the peer declared.
        declared: u32,
        /// The cap it exceeded.
        limit: u32,
    },
    /// A zero-length body. Nothing was consumed beyond the prefix, so the
    /// stream position is still known.
    ZeroLength,
    /// Underlying I/O failure (including a read timeout).
    Io(io::Error),
}

impl FrameError {
    /// Whether the reader has lost track of where the next frame starts.
    ///
    /// `true` means the connection must be closed: continuing would let a peer
    /// choose how the *next* bytes are framed. `false` means the caller may
    /// answer with an error frame and keep reading.
    #[must_use]
    pub fn stream_position_lost(&self) -> bool {
        match self {
            // The oversized body was never consumed; whatever follows it would
            // be interpreted as a length prefix. That is exactly the ambiguity
            // we refuse to have.
            FrameError::Oversize { .. } | FrameError::Truncated { .. } | FrameError::Io(_) => true,
            // Prefix consumed, no body: the next 4 bytes are still a prefix.
            FrameError::ZeroLength => false,
            FrameError::Eof => true,
        }
    }
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Eof => write!(f, "end of stream at frame boundary"),
            FrameError::Truncated { expected, received } => {
                write!(f, "truncated frame: expected {expected} bytes, got {received}")
            }
            FrameError::Oversize { declared, limit } => {
                write!(f, "frame length {declared} exceeds limit {limit}")
            }
            FrameError::ZeroLength => write!(f, "zero-length frame"),
            FrameError::Io(e) => write!(f, "frame io error: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Read exactly one frame body.
///
/// Returns [`FrameError::Eof`] only when the stream ends cleanly *before* any
/// byte of a length prefix.
pub fn read_frame<R: Read>(reader: &mut R, limit: u32) -> Result<Vec<u8>, FrameError> {
    let mut prefix = [0u8; LENGTH_PREFIX_BYTES];
    match read_full(reader, &mut prefix)? {
        0 => return Err(FrameError::Eof),
        n if n < LENGTH_PREFIX_BYTES => {
            return Err(FrameError::Truncated { expected: LENGTH_PREFIX_BYTES, received: n })
        }
        _ => {}
    }

    let declared = u32::from_be_bytes(prefix);
    if declared == 0 {
        return Err(FrameError::ZeroLength);
    }
    // Checked before the allocation below — this is the whole point of the cap.
    if declared > limit {
        return Err(FrameError::Oversize { declared, limit });
    }

    let expected = declared as usize;
    let mut body = vec![0u8; expected];
    let received = read_full(reader, &mut body)?;
    if received < expected {
        // Drop `body` unparsed. A partial frame is not data.
        return Err(FrameError::Truncated { expected, received });
    }
    Ok(body)
}

/// Write one frame body, refusing anything the reader would reject.
pub fn write_frame<W: Write>(writer: &mut W, body: &[u8], limit: u32) -> Result<(), FrameError> {
    if body.is_empty() {
        return Err(FrameError::ZeroLength);
    }
    let declared = u32::try_from(body.len())
        .map_err(|_| FrameError::Oversize { declared: u32::MAX, limit })?;
    if declared > limit {
        return Err(FrameError::Oversize { declared, limit });
    }
    writer.write_all(&declared.to_be_bytes())?;
    writer.write_all(body)?;
    writer.flush()?;
    Ok(())
}

/// `read_exact` that reports how much it got instead of losing the count, so a
/// truncated frame can be distinguished from a clean EOF.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize, FrameError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let Some(slice) = buf.get_mut(filled..) else { break };
        match reader.read(slice) {
            Ok(0) => break,
            Ok(n) => filled = filled.saturating_add(n),
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut v = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{}", MAX_FRAME_BYTES).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor, MAX_FRAME_BYTES).unwrap(), b"{}");
    }

    #[test]
    fn clean_eof_at_boundary() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        assert!(matches!(read_frame(&mut cursor, MAX_FRAME_BYTES), Err(FrameError::Eof)));
    }

    #[test]
    fn truncated_body_is_not_returned() {
        let mut bytes = framed(b"{\"a\":1}");
        bytes.truncate(6);
        let mut cursor = std::io::Cursor::new(bytes);
        let err = read_frame(&mut cursor, MAX_FRAME_BYTES).unwrap_err();
        assert!(matches!(err, FrameError::Truncated { expected: 7, received: 2 }));
        assert!(err.stream_position_lost());
    }

    #[test]
    fn truncated_prefix_is_truncation_not_eof() {
        let mut cursor = std::io::Cursor::new(vec![0u8, 0u8]);
        assert!(matches!(
            read_frame(&mut cursor, MAX_FRAME_BYTES),
            Err(FrameError::Truncated { expected: 4, received: 2 })
        ));
    }

    #[test]
    fn oversize_is_rejected_without_allocating() {
        let mut bytes = u32::MAX.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"nope");
        let mut cursor = std::io::Cursor::new(bytes);
        let err = read_frame(&mut cursor, MAX_FRAME_BYTES).unwrap_err();
        assert!(matches!(err, FrameError::Oversize { declared: u32::MAX, .. }));
        assert!(err.stream_position_lost());
    }

    #[test]
    fn zero_length_keeps_stream_position() {
        let mut cursor = std::io::Cursor::new(0u32.to_be_bytes().to_vec());
        let err = read_frame(&mut cursor, MAX_FRAME_BYTES).unwrap_err();
        assert!(matches!(err, FrameError::ZeroLength));
        assert!(!err.stream_position_lost());
    }

    #[test]
    fn pipelined_frames_read_in_order() {
        let mut bytes = framed(b"{\"n\":1}");
        bytes.extend_from_slice(&framed(b"{\"n\":2}"));
        let mut cursor = std::io::Cursor::new(bytes);
        assert_eq!(read_frame(&mut cursor, MAX_FRAME_BYTES).unwrap(), b"{\"n\":1}");
        assert_eq!(read_frame(&mut cursor, MAX_FRAME_BYTES).unwrap(), b"{\"n\":2}");
        assert!(matches!(read_frame(&mut cursor, MAX_FRAME_BYTES), Err(FrameError::Eof)));
    }
}
