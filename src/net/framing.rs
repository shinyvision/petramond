//! TCP frame codec: `[u32 LE len][u8 flags][body]`,
//! body = postcard. Flag bit 0 marks a zlib-compressed body — applied when the
//! serialized body exceeds `COMPRESS_MIN` and compression actually shrinks
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
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Bodies larger than this are candidates for zlib compression.
const COMPRESS_MIN: usize = 1024;

/// Frame flag bit 0: the body is zlib-compressed.
const FLAG_ZLIB: u8 = 1;

fn invalid<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Encode `msg` as one complete frame (header + body).
pub fn encode_frame<T: Serialize>(msg: &T) -> io::Result<Vec<u8>> {
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
    let mut frame = Vec::with_capacity(HEADER_LEN + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.push(flags);
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Encode `msg` as one frame and write it with a single `write_all` (one
/// packet under NODELAY). Flushing is the caller's concern.
pub fn write_msg<T: Serialize, W: Write>(w: &mut W, msg: &T) -> io::Result<()> {
    w.write_all(&encode_frame(msg)?)
}

/// Frame header: `[u32 LE len][u8 flags]`.
const HEADER_LEN: usize = 5;

fn parse_header(header: &[u8], max_body: usize) -> io::Result<(usize, u8)> {
    let len = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes")) as usize;
    if len > max_body {
        return Err(invalid(format!("oversize frame ({len} bytes)")));
    }
    Ok((len, header[4]))
}

/// Decode one frame body, inflating it when flagged — the decompressed size
/// is capped at `max_body` as well (zlib-bomb guard).
fn decode_body<T: DeserializeOwned>(flags: u8, body: &[u8], max_body: usize) -> io::Result<T> {
    if flags & FLAG_ZLIB == 0 {
        return postcard::from_bytes(body).map_err(invalid);
    }
    // Read at most one byte past the cap so overflow is detected, never
    // materialized.
    let mut dec = flate2::read::ZlibDecoder::new(body).take(max_body as u64 + 1);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)?;
    if out.len() > max_body {
        return Err(invalid("oversize decompressed frame"));
    }
    postcard::from_bytes(&out).map_err(invalid)
}

/// Read one frame and decode it. Errors are terminal for the connection:
/// `InvalidData` for oversize/undecodable frames, the underlying I/O error for
/// EOF/timeout/reset. Reads exactly the frame's bytes (no over-read), so a
/// handshake over the raw stream can hand off to a buffered reader safely.
pub fn read_msg<T: DeserializeOwned, R: Read>(r: &mut R) -> io::Result<T> {
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header)?;
    let (len, flags) = parse_header(&header, MAX_FRAME)?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    decode_body(flags, &body, MAX_FRAME)
}

/// Decode the first frame at the front of `buf` without blocking — the
/// incremental twin of [`read_msg`] for sockets read in nonblocking chunks.
/// `Ok(None)` = the frame is still incomplete; `Ok(Some((msg, used)))` also
/// reports how many bytes of `buf` the frame occupied. Frames (and their
/// decompressed bodies) larger than `max_body` are `InvalidData` as soon as
/// the header arrives, so a hostile peer can never make the caller buffer
/// more than `max_body` bytes.
pub fn decode_frame<T: DeserializeOwned>(
    buf: &[u8],
    max_body: usize,
) -> io::Result<Option<(T, usize)>> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let max_body = max_body.min(MAX_FRAME);
    let (len, flags) = parse_header(&buf[..HEADER_LEN], max_body)?;
    let end = HEADER_LEN + len;
    if buf.len() < end {
        return Ok(None);
    }
    let msg = decode_body(flags, &buf[HEADER_LEN..end], max_body)?;
    Ok(Some((msg, end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::{ClientToServer, SectionBlocks, ServerToClient};

    fn frame_of<T: Serialize>(msg: &T) -> Vec<u8> {
        let mut out = Vec::new();
        write_msg(&mut out, msg).expect("frame writes");
        out
    }

    #[test]
    fn small_messages_roundtrip_uncompressed() {
        let msg = ClientToServer::Join {
            player_name: "Rachel".into(),
            key: crate::net::identity::PlayerKey([1; 32]),
            proof: vec![2; 64],
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
        let blocks: Vec<u16> = (0..4096u32).map(|i| (i / 512) as u16).collect();
        let msg = ServerToClient::SectionData(Box::new(crate::net::protocol::SectionPayload {
            pos: petramond_world::chunk::SectionPos::new(1, 4, -2),
            blocks: SectionBlocks(std::sync::Arc::from(blocks.into_boxed_slice())),
            metrics: Default::default(),
            fluid: None,
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

    #[test]
    fn incremental_decode_waits_for_whole_frames_and_caps_their_size() {
        let msg = ClientToServer::Hello { protocol: 7 };
        let mut stream = encode_frame(&msg).expect("encodes");
        let one = stream.len();
        stream.extend(encode_frame(&ClientToServer::KeepAlive).expect("encodes"));
        for cut in 0..one {
            assert!(
                decode_frame::<ClientToServer>(&stream[..cut], 64)
                    .expect("a partial frame is not an error")
                    .is_none(),
                "{cut} bytes are not a frame yet"
            );
        }
        let (first, used) = decode_frame::<ClientToServer>(&stream, 64)
            .expect("decodes")
            .expect("complete");
        assert_eq!((first, used), (msg, one));
        let (second, _) = decode_frame::<ClientToServer>(&stream[used..], 64)
            .expect("decodes")
            .expect("complete");
        assert_eq!(second, ClientToServer::KeepAlive);

        // A header announcing more than the cap fails before its body exists.
        let mut header = 65u32.to_le_bytes().to_vec();
        header.push(0);
        let err = decode_frame::<ClientToServer>(&header, 64).expect_err("capped");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
