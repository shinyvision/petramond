use super::{bytes::Reader, palette, voxels};
use crate::schematic::{SchematicSection, SectionCell};
use std::io::{BufReader, Read, Write};

pub(super) const PREFIX_SIZE: usize = 36;
/// Largest decompressed section body. A section is at most 4096 cells; only
/// thousands of distinct data-heavy palette entries approach this.
const MAX_RAW_BYTES: u64 = 64 * 1024 * 1024;
/// Deflate can grow incompressible input slightly.
const MAX_COMPRESSED_BYTES: u64 = MAX_RAW_BYTES + (MAX_RAW_BYTES >> 10) + 64;

pub(crate) fn encode(section: &SchematicSection) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    palette::encode(&section.palette.iter().collect::<Vec<_>>(), &mut body);
    let cells: Vec<_> = section
        .cells
        .iter()
        .map(|c| (usize::from(c.index), usize::from(c.palette)))
        .collect();
    body.extend(voxels::encode(&cells, section.palette.len(), 4096));
    let raw_len = body.len() as u64;
    if raw_len > MAX_RAW_BYTES {
        return Err("Schematic section is too large".into());
    }
    let mut compressor = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    compressor.write_all(&body).map_err(|e| e.to_string())?;
    let compressed = compressor.finish().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(PREFIX_SIZE + compressed.len());
    for p in section.pos {
        out.extend_from_slice(&p.to_le_bytes());
    }
    out.extend_from_slice(&(section.cells.len() as u32).to_le_bytes());
    out.extend_from_slice(&raw_len.to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
    let mut hash = crc32fast::Hasher::new();
    hash.update(&out);
    hash.update(&compressed);
    out.extend_from_slice(&hash.finalize().to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

pub(crate) fn decode(input: &mut impl Read, size: [i32; 3]) -> Result<SchematicSection, String> {
    let mut prefix = [0; PREFIX_SIZE];
    input.read_exact(&mut prefix).map_err(|e| e.to_string())?;
    let n = |i| u32::from_le_bytes(prefix[i..i + 4].try_into().unwrap());
    let wide = |i| u64::from_le_bytes(prefix[i..i + 8].try_into().unwrap());
    let pos = [n(0) as i32, n(4) as i32, n(8) as i32];
    let count = n(12) as usize;
    let raw_len = wide(16);
    let compressed_len = wide(24);
    let crc = n(32);
    if !(1..=4096).contains(&count)
        || raw_len > MAX_RAW_BYTES
        || compressed_len > MAX_COMPRESSED_BYTES
        || size.iter().any(|n| *n <= 0)
        || (0..3).any(|a| pos[a] < 0 || pos[a] > (size[a] - 1) / 16)
    {
        return Err("Invalid schematic section bounds".into());
    }
    let mut checksum = crc32fast::Hasher::new();
    checksum.update(&prefix[..32]);
    let hash = HashReader {
        input: input.take(compressed_len),
        hash: checksum,
    };
    let mut decoder = flate2::bufread::ZlibDecoder::new(BufReader::new(hash));
    // The declared length bounds the inflation itself, so a small stream
    // that inflates without end is cut off rather than followed.
    let mut reader = Reader::new((&mut decoder).take(raw_len));
    let palette = palette::decode(&mut reader, count)?;
    let cells = voxels::decode(&mut reader, count, palette.len(), 4096)?;
    reader.finish()?;
    Reader::new(&mut decoder).finish()?;
    if decoder.total_out() != raw_len || decoder.total_in() != compressed_len {
        return Err("Schematic section length mismatch".into());
    }
    if decoder.into_inner().into_inner().hash.finalize() != crc {
        return Err("Schematic section checksum mismatch".into());
    }
    let section = SchematicSection {
        pos,
        palette,
        cells: cells
            .into_iter()
            .map(|(index, palette)| SectionCell {
                index: index as u16,
                palette: palette as u16,
            })
            .collect(),
    };
    section.validate(size)?;
    Ok(section)
}

struct HashReader<R> {
    input: R,
    hash: crc32fast::Hasher,
}
impl<R: Read> Read for HashReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.input.read(out)?;
        self.hash.update(&out[..n]);
        Ok(n)
    }
}
