use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::secure::{FrameOpener, FrameSealer, TAG_LEN};

pub const MAX_FRAME: usize = 8 * 1024 * 1024;

const COMPRESS_MIN: usize = 1024;

const FLAG_ZLIB: u8 = 1;

const FLAG_SEALED: u8 = 2;

fn invalid<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

pub fn encode_frame<T: Serialize>(msg: &T) -> io::Result<Vec<u8>> {
    let (flags, body) = plain_body(msg)?;
    let mut frame = Vec::with_capacity(HEADER_LEN + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.push(flags);
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// A frame for a keyed connection: the body is encrypted and the header is authenticated.
pub fn encode_sealed<T: Serialize>(msg: &T, sealer: &mut FrameSealer) -> io::Result<Vec<u8>> {
    let (flags, body) = plain_body(msg)?;
    let mut frame = Vec::with_capacity(HEADER_LEN + body.len() + TAG_LEN);
    frame.extend_from_slice(&((body.len() + TAG_LEN) as u32).to_le_bytes());
    frame.push(flags | FLAG_SEALED);
    let mut sealed = body;
    sealer.seal(&frame[..HEADER_LEN], &mut sealed)?;
    frame.extend_from_slice(&sealed);
    Ok(frame)
}

pub fn write_sealed<T: Serialize, W: Write>(
    w: &mut W,
    msg: &T,
    sealer: &mut FrameSealer,
) -> io::Result<()> {
    w.write_all(&encode_sealed(msg, sealer)?)
}

/// Per-thread codec state. A deflate or inflate state is a few hundred KiB of tables, so it is
/// reset between frames, never rebuilt, and the buffers keep their (initialized) capacity.
struct FrameCodec {
    body: Vec<u8>,
    packed: Vec<u8>,
    inflated: Vec<u8>,
    deflate: Option<flate2::Compress>,
    inflate: Option<flate2::Decompress>,
}

thread_local! {
    static CODEC: std::cell::RefCell<FrameCodec> = const {
        std::cell::RefCell::new(FrameCodec {
            body: Vec::new(),
            packed: Vec::new(),
            inflated: Vec::new(),
            deflate: None,
            inflate: None,
        })
    };
}

fn plain_body<T: Serialize>(msg: &T) -> io::Result<(u8, Vec<u8>)> {
    CODEC.with(|codec| {
        let mut codec = codec.borrow_mut();
        let FrameCodec {
            body,
            packed,
            deflate,
            ..
        } = &mut *codec;
        body.clear();
        *body = postcard::to_extend(msg, std::mem::take(body)).map_err(invalid)?;
        if body.len() > MAX_FRAME {
            return Err(invalid(format!("oversize frame ({} bytes)", body.len())));
        }
        if body.len() > COMPRESS_MIN {
            let deflate = deflate
                .get_or_insert_with(|| flate2::Compress::new(flate2::Compression::fast(), true));
            deflate.reset();
            // Only a strictly smaller result ships compressed, so the output never needs more
            // room than the input.
            if packed.len() < body.len() {
                packed.resize(body.len(), 0);
            }
            let status = deflate
                .compress(
                    body,
                    &mut packed[..body.len()],
                    flate2::FlushCompress::Finish,
                )
                .map_err(invalid)?;
            let written = deflate.total_out() as usize;
            if status == flate2::Status::StreamEnd && written < body.len() {
                return Ok((FLAG_ZLIB, packed[..written].to_vec()));
            }
        }
        Ok((0, body.clone()))
    })
}

pub fn write_msg<T: Serialize, W: Write>(w: &mut W, msg: &T) -> io::Result<()> {
    w.write_all(&encode_frame(msg)?)
}

const HEADER_LEN: usize = 5;

fn parse_header(header: &[u8], max_body: usize, sealed: bool) -> io::Result<(usize, u8)> {
    let len = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes")) as usize;
    let flags = header[4];
    if (flags & FLAG_SEALED != 0) != sealed {
        return Err(invalid(if sealed {
            "unsealed frame on a keyed connection"
        } else {
            "sealed frame before key agreement"
        }));
    }
    let max_len = if sealed { max_body + TAG_LEN } else { max_body };
    if len > max_len || (sealed && len < TAG_LEN) {
        return Err(invalid(format!("oversize frame ({len} bytes)")));
    }
    Ok((len, flags & !FLAG_SEALED))
}

fn open_body(
    header: &[u8],
    mut body: Vec<u8>,
    opener: Option<&mut FrameOpener>,
) -> io::Result<Vec<u8>> {
    if let Some(opener) = opener {
        let len = opener.open(header, &mut body)?;
        body.truncate(len);
    }
    Ok(body)
}

fn decode_body<T: DeserializeOwned>(flags: u8, body: &[u8], max_body: usize) -> io::Result<T> {
    if flags & FLAG_ZLIB == 0 {
        return postcard::from_bytes(body).map_err(invalid);
    }
    CODEC.with(|codec| {
        let mut codec = codec.borrow_mut();
        let FrameCodec {
            inflated, inflate, ..
        } = &mut *codec;
        let inflate = inflate.get_or_insert_with(|| flate2::Decompress::new(true));
        inflate.reset(true);
        let limit = max_body + 1;
        if inflated.len() < limit.min(body.len().saturating_mul(4).max(64 * 1024)) {
            inflated.resize(limit.min(body.len().saturating_mul(4).max(64 * 1024)), 0);
        }
        loop {
            let consumed = inflate.total_in() as usize;
            let produced = inflate.total_out() as usize;
            let status = inflate
                .decompress(
                    &body[consumed..],
                    &mut inflated[produced..],
                    flate2::FlushDecompress::Finish,
                )
                .map_err(invalid)?;
            let produced_now = inflate.total_out() as usize;
            if produced_now > max_body {
                return Err(invalid("oversize decompressed frame"));
            }
            if status == flate2::Status::StreamEnd {
                return postcard::from_bytes(&inflated[..produced_now]).map_err(invalid);
            }
            if produced_now == inflated.len() {
                if inflated.len() >= limit {
                    return Err(invalid("oversize decompressed frame"));
                }
                inflated.resize((inflated.len() * 2).min(limit), 0);
            } else if inflate.total_in() as usize == consumed && produced_now == produced {
                return Err(invalid("truncated compressed frame"));
            }
        }
    })
}

pub fn read_msg<T: DeserializeOwned, R: Read>(r: &mut R) -> io::Result<T> {
    read_msg_bounded(r, MAX_FRAME).map(|(msg, _)| msg)
}

pub fn read_msg_bounded<T: DeserializeOwned, R: Read>(
    r: &mut R,
    max_body: usize,
) -> io::Result<(T, usize)> {
    read_frame(r, max_body, None)
}

/// Reads the next frame of a keyed connection; it must be sealed and must open.
pub fn read_opened<T: DeserializeOwned, R: Read>(
    r: &mut R,
    max_body: usize,
    opener: &mut FrameOpener,
) -> io::Result<(T, usize)> {
    read_frame(r, max_body, Some(opener))
}

fn read_frame<T: DeserializeOwned, R: Read>(
    r: &mut R,
    max_body: usize,
    opener: Option<&mut FrameOpener>,
) -> io::Result<(T, usize)> {
    let max_body = max_body.min(MAX_FRAME);
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header)?;
    let (len, flags) = parse_header(&header, max_body, opener.is_some())?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let body = open_body(&header, body, opener)?;
    Ok((decode_body(flags, &body, max_body)?, HEADER_LEN + len))
}

pub fn decode_frame<T: DeserializeOwned>(
    buf: &[u8],
    max_body: usize,
) -> io::Result<Option<(T, usize)>> {
    take_frame(buf, max_body, None)
}

/// [`decode_frame`] for a keyed connection. The opener only advances once a whole frame is in.
pub fn decode_opened<T: DeserializeOwned>(
    buf: &[u8],
    max_body: usize,
    opener: &mut FrameOpener,
) -> io::Result<Option<(T, usize)>> {
    take_frame(buf, max_body, Some(opener))
}

fn take_frame<T: DeserializeOwned>(
    buf: &[u8],
    max_body: usize,
    opener: Option<&mut FrameOpener>,
) -> io::Result<Option<(T, usize)>> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let max_body = max_body.min(MAX_FRAME);
    let (len, flags) = parse_header(&buf[..HEADER_LEN], max_body, opener.is_some())?;
    let end = HEADER_LEN + len;
    if buf.len() < end {
        return Ok(None);
    }
    let body = open_body(&buf[..HEADER_LEN], buf[HEADER_LEN..end].to_vec(), opener)?;
    let msg = decode_body(flags, &body, max_body)?;
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
            credential: crate::net::protocol::JoinCredential::Ticket("a-join-ticket".into()),
            key: crate::net::identity::PlayerKey([1; 32]),
            proof: vec![2; 64],
            view_distance: 16,
            cached_sections: Vec::new(),
        };
        let frame = frame_of(&msg);
        assert_eq!(frame[4], 0, "a tiny body ships raw (no zlib flag)");
        let back: ClientToServer = read_msg(&mut &frame[..]).expect("decodes");
        assert_eq!(back, msg);

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
        let huge = ServerToClient::Disconnect {
            reason: "x".repeat(MAX_FRAME + 1),
        };
        let mut out = Vec::new();
        let err = write_msg(&mut out, &huge).expect_err("oversize write rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(out.is_empty(), "nothing was written");

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

        let mut header = 65u32.to_le_bytes().to_vec();
        header.push(0);
        let err = decode_frame::<ClientToServer>(&header, 64).expect_err("capped");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn sealed_frames_roundtrip_and_refuse_to_mix_with_plain_ones() {
        let (mut client, mut server) = crate::net::secure::channels_for_test();
        let big = ClientToServer::ChatSend {
            text: "z".repeat(4096),
        };
        let mut stream = Vec::new();
        for msg in [&ClientToServer::KeepAlive, &big] {
            write_sealed(&mut stream, msg, &mut client.sealer).expect("seals");
        }
        assert_eq!(stream[4] & FLAG_SEALED, FLAG_SEALED);
        let mut r = &stream[..];
        let (a, _) =
            read_opened::<ClientToServer, _>(&mut r, 64 * 1024, &mut server.opener).expect("opens");
        let (b, _) = decode_opened::<ClientToServer>(r, 64 * 1024, &mut server.opener)
            .expect("opens")
            .expect("complete");
        assert_eq!((a, b), (ClientToServer::KeepAlive, big));

        let sealed = encode_sealed(&ClientToServer::KeepAlive, &mut client.sealer).unwrap();
        let err = read_msg::<ClientToServer, _>(&mut &sealed[..]).expect_err("plain reader");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let plain = encode_frame(&ClientToServer::KeepAlive).unwrap();
        let err = read_opened::<ClientToServer, _>(&mut &plain[..], 1024, &mut server.opener)
            .expect_err("keyed reader");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_forged_sealed_frame_is_refused() {
        let (mut client, mut server) = crate::net::secure::channels_for_test();
        let mut frame = encode_sealed(&ClientToServer::KeepAlive, &mut client.sealer).unwrap();
        let last = frame.len() - 1;
        frame[last] ^= 0x80;
        let err = read_opened::<ClientToServer, _>(&mut &frame[..], 1024, &mut server.opener)
            .expect_err("tampered");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
