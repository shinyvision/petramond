use super::{
    bytes::{self, Reader},
    MAX_THUMBNAIL_BYTES, MAX_THUMBNAIL_SIDE,
};

const MAGIC: &[u8; 8] = b"LLSCHEM\0";
const VERSION: u16 = 2;
pub const HEADER_SIZE: usize = 48;
/// Longest side of a design, in blocks. Real designs are a few hundred.
pub const MAX_AXIS: i32 = 1024;
/// Most stored cells in one design (a solid 256³).
pub const MAX_CELLS: usize = 1 << 24;
const MAX_METADATA_BYTES: usize = 256;
const MAX_NAME_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub name: String,
    pub size: [i32; 3],
    pub cell_count: usize,
    pub thumbnail_size: [u32; 2],
}
impl Metadata {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        bytes::blob(&mut out, self.name.as_bytes());
        for n in self.size {
            bytes::uint(&mut out, n as usize);
        }
        bytes::uint(&mut out, self.cell_count);
        for n in self.thumbnail_size {
            bytes::uint(&mut out, n as usize);
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct Header {
    pub metadata_len: usize,
    pub thumbnail_len: usize,
    pub payload_len: u64,
    pub section_count: u64,
    metadata_crc: u32,
    pub thumbnail_crc: u32,
    pub revision: u32,
}
impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != HEADER_SIZE || &bytes[..8] != MAGIC {
            return Err("Not an .llschematic file".into());
        }
        let n = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let wide = |i| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        if crc32fast::hash(&bytes[..44]) != n(44) {
            return Err("Schematic header checksum mismatch".into());
        }
        if u16::from_le_bytes(bytes[8..10].try_into().unwrap()) != VERSION
            || bytes[10..12] != [0, 0]
        {
            return Err("Unsupported .llschematic version".into());
        }
        let h = Self {
            metadata_len: n(12) as usize,
            thumbnail_len: n(16) as usize,
            payload_len: wide(20),
            section_count: wide(28),
            metadata_crc: n(36),
            thumbnail_crc: n(40),
            revision: n(44),
        };
        if !(1..=MAX_METADATA_BYTES).contains(&h.metadata_len)
            || !(1..=MAX_THUMBNAIL_BYTES).contains(&h.thumbnail_len)
            || h.payload_len > super::MAX_ARCHIVE_BYTES
            || h.section_count == 0
            || h.section_count > h.payload_len / super::section::PREFIX_SIZE as u64
        {
            return Err("Invalid schematic section lengths".into());
        }
        Ok(h)
    }
    pub fn thumbnail_offset(&self) -> usize {
        HEADER_SIZE + self.metadata_len
    }
    pub fn payload_offset(&self) -> usize {
        self.thumbnail_offset() + self.thumbnail_len
    }
    pub fn check_file_len(&self, len: u64) -> Result<(), String> {
        if self.payload_len.checked_add(self.payload_offset() as u64) == Some(len) {
            Ok(())
        } else {
            Err("Schematic file length mismatch".into())
        }
    }
    pub fn metadata(&self, bytes: &[u8]) -> Result<Metadata, String> {
        if bytes.len() != self.metadata_len || crc32fast::hash(bytes) != self.metadata_crc {
            return Err("Schematic metadata checksum mismatch".into());
        }
        let mut r = Reader::new(bytes);
        let name =
            String::from_utf8(r.blob(MAX_NAME_BYTES)?).map_err(|_| "Invalid schematic title")?;
        let size = [
            r.uint(MAX_AXIS as usize)? as i32,
            r.uint(MAX_AXIS as usize)? as i32,
            r.uint(MAX_AXIS as usize)? as i32,
        ];
        let cell_count = r.uint(MAX_CELLS)?;
        let thumbnail_size = [
            r.uint(MAX_THUMBNAIL_SIDE as usize)? as u32,
            r.uint(MAX_THUMBNAIL_SIDE as usize)? as u32,
        ];
        r.finish()?;
        let volume = size.iter().map(|n| *n as u128).product::<u128>();
        if name.trim().is_empty()
            || size.contains(&0)
            || thumbnail_size.contains(&0)
            || cell_count == 0
            || cell_count as u128 > volume
            || self.section_count > cell_count as u64
            || cell_count as u128 > u128::from(self.section_count) * 4096
        {
            return Err("Invalid schematic metadata".into());
        }
        Ok(Metadata {
            name,
            size,
            cell_count,
            thumbnail_size,
        })
    }
    pub fn check_thumbnail(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() == self.thumbnail_len && crc32fast::hash(bytes) == self.thumbnail_crc {
            Ok(())
        } else {
            Err("Schematic thumbnail checksum mismatch".into())
        }
    }
}

pub(super) fn encode(
    metadata: &[u8],
    thumbnail: &[u8],
    payload_len: u64,
    section_count: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    out.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    out.extend_from_slice(&(thumbnail.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(&(section_count as u64).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(metadata).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(thumbnail).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(&out).to_le_bytes());
    out
}
