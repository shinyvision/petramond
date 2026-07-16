//! TCP frame codec: `[u32 LE len][u8 flags][body]`,
//! body = postcard. Flag bit 0 marks a zlib-compressed body — applied when the
//! serialized body exceeds [`COMPRESS_MIN`] and compression actually shrinks
//! it (terrain `SectionData`/`ColumnData` mainly; tick batches stay raw).
//!
//! Frames are bounded by [`MAX_FRAME`] in BOTH directions and on BOTH sides of
//! the compressor (an oversize length is a protocol error; the caller drops
//! the connection). No legitimate message is anywhere near the cap — sections
//! are ~20 KiB — so the bound only exists to stop hostile/corrupt streams.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Hard cap on a frame body (and on its decompressed size): 8 MiB.
pub(crate) const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Bodies larger than this are candidates for zlib compression.
const COMPRESS_MIN: usize = 1024;

/// Frame flag bit 0: the body is zlib-compressed.
const FLAG_ZLIB: u8 = 1;

fn invalid<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Encode `msg` as one frame and write it with a single `write_all` (one
/// packet under NODELAY). Flushing is the caller's concern.
pub(crate) fn write_msg<T: Serialize, W: Write>(w: &mut W, msg: &T) -> io::Result<()> {
    let body = postcard::to_allocvec(msg).map_err(invalid)?;
    if body.len() > MAX_FRAME {
        return Err(invalid(format!("oversize frame ({} bytes)", body.len())));
    }
    let (flags, body) = if body.len() > COMPRESS_MIN {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&body)?;
        let compressed = enc.finish()?;
        if compressed.len() < body.len() {
            (FLAG_ZLIB, compressed)
        } else {
            (0, body)
        }
    } else {
        (0, body)
    };
    let mut frame = Vec::with_capacity(5 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.push(flags);
    frame.extend_from_slice(&body);
    w.write_all(&frame)
}

/// Read one frame and decode it. Errors are terminal for the connection:
/// `InvalidData` for oversize/undecodable frames, the underlying I/O error for
/// EOF/timeout/reset. Reads exactly the frame's bytes (no over-read), so a
/// handshake over the raw stream can hand off to a buffered reader safely.
pub(crate) fn read_msg<T: DeserializeOwned, R: Read>(r: &mut R) -> io::Result<T> {
    let mut header = [0u8; 5];
    r.read_exact(&mut header)?;
    let len = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes")) as usize;
    let flags = header[4];
    if len > MAX_FRAME {
        return Err(invalid(format!("oversize frame ({len} bytes)")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let decompressed;
    let bytes: &[u8] = if flags & FLAG_ZLIB != 0 {
        // Cap the decompressed size too (zlib-bomb guard): read at most one
        // byte past the cap so overflow is detected, never materialized.
        let mut dec = flate2::read::ZlibDecoder::new(&body[..]).take(MAX_FRAME as u64 + 1);
        let mut out = Vec::new();
        dec.read_to_end(&mut out)?;
        if out.len() > MAX_FRAME {
            return Err(invalid("oversize decompressed frame"));
        }
        decompressed = out;
        &decompressed
    } else {
        &body
    };
    postcard::from_bytes(bytes).map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::{ClientToServer, SectionBytes, ServerToClient};

    fn frame_of<T: Serialize>(msg: &T) -> Vec<u8> {
        let mut out = Vec::new();
        write_msg(&mut out, msg).expect("frame writes");
        out
    }

    #[test]
    fn small_messages_roundtrip_uncompressed() {
        let msg = ClientToServer::Join {
            player_name: "Rachel".into(),
            view_distance: 16,
            cached_sections: Vec::new(),
        };
        let frame = frame_of(&msg);
        assert_eq!(frame[4], 0, "a tiny body ships raw (no zlib flag)");
        let back: ClientToServer = read_msg(&mut &frame[..]).expect("decodes");
        assert_eq!(back, msg);

        // Two frames back-to-back read in order without over-reading.
        let second = ClientToServer::KeepAlive;
        let mut stream = frame.clone();
        stream.extend(frame_of(&second));
        let mut r = &stream[..];
        let a: ClientToServer = read_msg(&mut r).expect("first");
        let b: ClientToServer = read_msg(&mut r).expect("second");
        assert_eq!(a, msg);
        assert_eq!(b, second);
        assert!(r.is_empty(), "nothing consumed beyond the two frames");
    }

    #[test]
    fn large_section_payloads_ship_zlib_compressed_and_roundtrip() {
        let blocks: Vec<u8> = (0..4096u32).map(|i| (i / 512) as u8).collect();
        let msg = ServerToClient::SectionData(Box::new(crate::net::protocol::SectionPayload {
            pos: crate::chunk::SectionPos::new(1, 4, -2),
            blocks: SectionBytes(std::sync::Arc::from(blocks.into_boxed_slice())),
            metrics: Default::default(),
            water: None,
            skylight: None,
            blocklight: None,
            states: Default::default(),
        }));
        let frame = frame_of(&msg);
        assert_eq!(frame[4] & FLAG_ZLIB, FLAG_ZLIB, "a 4 KiB body compresses");
        let len = u32::from_le_bytes(frame[0..4].try_into().unwrap()) as usize;
        assert!(len < 4096, "the wire body is smaller than the raw payload");
        assert_eq!(frame.len(), 5 + len);
        let back: ServerToClient = read_msg(&mut &frame[..]).expect("decodes");
        assert_eq!(back, msg);
    }

    #[test]
    fn oversize_frames_are_rejected_on_both_sides() {
        // Write side: a body beyond MAX_FRAME is a protocol error before any
        // compression could hide it.
        let huge = ServerToClient::Disconnect {
            reason: "x".repeat(MAX_FRAME + 1),
        };
        let mut out = Vec::new();
        let err = write_msg(&mut out, &huge).expect_err("oversize write rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(out.is_empty(), "nothing was written");

        // Read side: an oversize length rejects on the header alone.
        let mut header = Vec::new();
        header.extend_from_slice(&((MAX_FRAME as u32) + 1).to_le_bytes());
        header.push(0);
        let err =
            read_msg::<ClientToServer, _>(&mut &header[..]).expect_err("oversize read rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
