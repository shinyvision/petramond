//! Independently compressed sections and an embedded preview, indexed without decoding blocks.
mod bytes;
mod header;
mod palette;
pub(crate) mod section;
#[cfg(test)]
mod tests;
mod voxels;

use super::Schematic;
pub use header::{Header, Metadata, HEADER_SIZE, MAX_AXIS, MAX_CELLS};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

pub const EXTENSION: &str = "llschematic";
/// Largest whole archive accepted, from a file or a network stream. Far
/// above any real design; it bounds what a hostile length can make a
/// reader buffer.
pub const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_THUMBNAIL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_THUMBNAIL_SIDE: u32 = 512;

pub fn encode(schematic: &Schematic, thumbnail: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    write_to(&mut out, schematic, thumbnail)?;
    Ok(out.into_inner())
}

/// An archive for the wire alone: a library file's preview is its owner's to
/// draw, so this one carries a blank.
pub fn encode_bare(schematic: &Schematic) -> Result<Vec<u8>, String> {
    static BLANK: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
        let mut png = Cursor::new(Vec::new());
        image::RgbaImage::new(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("a blank pixel encodes");
        png.into_inner()
    });
    encode(schematic, &BLANK)
}

/// The file writer holds only one encoded section in addition to the schematic.
pub fn write_to(
    out: &mut (impl Write + Seek),
    schematic: &Schematic,
    thumbnail: &[u8],
) -> Result<(), String> {
    schematic.validate()?;
    // What a reader would refuse is refused here, before it becomes a file.
    if schematic.size.iter().any(|n| *n > MAX_AXIS) || schematic.cell_count() > MAX_CELLS {
        return Err("Schematic is too large".into());
    }
    let preview = decode_thumbnail(thumbnail)?;
    let meta = Metadata {
        name: schematic.name.clone(),
        size: schematic.size,
        cell_count: schematic.cell_count(),
        thumbnail_size: [preview.width, preview.height],
    }
    .encode();
    let start = out.stream_position().map_err(|e| e.to_string())?;
    out.write_all(&[0; HEADER_SIZE])
        .map_err(|e| e.to_string())?;
    out.write_all(&meta).map_err(|e| e.to_string())?;
    out.write_all(thumbnail).map_err(|e| e.to_string())?;
    let mut payload_len = 0u64;
    for section in &schematic.sections {
        let bytes = section::encode(section)?;
        out.write_all(&bytes).map_err(|e| e.to_string())?;
        payload_len = payload_len
            .checked_add(bytes.len() as u64)
            .ok_or("Schematic file length overflow")?;
    }
    let end = out.stream_position().map_err(|e| e.to_string())?;
    out.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    out.write_all(&header::encode(
        &meta,
        thumbnail,
        payload_len,
        schematic.sections.len(),
    ))
    .map_err(|e| e.to_string())?;
    out.seek(SeekFrom::Start(end)).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn decode(bytes: &[u8]) -> Result<Schematic, String> {
    let h = Header::parse(
        bytes
            .get(..HEADER_SIZE)
            .ok_or("Truncated schematic header")?,
    )?;
    h.check_file_len(bytes.len() as u64)?;
    let meta = h.metadata(&bytes[HEADER_SIZE..h.thumbnail_offset()])?;
    h.check_thumbnail(&bytes[h.thumbnail_offset()..h.payload_offset()])?;
    decode_payload(&h, &meta, &mut &bytes[h.payload_offset()..])
}

pub(super) fn decode_payload(
    h: &Header,
    meta: &Metadata,
    input: &mut impl Read,
) -> Result<Schematic, String> {
    let mut input = input.take(h.payload_len);
    let mut sections = Vec::new();
    let mut count = 0usize;
    for _ in 0..h.section_count {
        let section = section::decode(&mut input, meta.size)?;
        count = count
            .checked_add(section.cells.len())
            .ok_or("Schematic cell count overflow")?;
        if count > meta.cell_count
            || sections
                .last()
                .is_some_and(|s: &super::SchematicSection| s.pos >= section.pos)
        {
            return Err("Invalid schematic section index".into());
        }
        sections.push(section);
    }
    if count != meta.cell_count || input.limit() != 0 {
        return Err("Schematic payload length mismatch".into());
    }
    let schematic = Schematic {
        name: meta.name.clone(),
        size: meta.size,
        sections,
    };
    schematic.validate()?;
    Ok(schematic)
}

#[derive(Clone)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: std::sync::Arc<[u8]>,
}

/// Decode a bounded embedded PNG on a worker, never during UI painting.
pub fn decode_thumbnail(png: &[u8]) -> Result<Thumbnail, String> {
    if png.len() > MAX_THUMBNAIL_BYTES {
        return Err("Schematic thumbnail is too large".into());
    }
    let reader = || {
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_THUMBNAIL_SIDE);
        limits.max_image_height = Some(MAX_THUMBNAIL_SIDE);
        limits.max_alloc = Some(8 * 1024 * 1024);
        reader.limits(limits);
        reader
    };
    let (width, height) = reader().into_dimensions().map_err(|e| e.to_string())?;
    if width == 0 || height == 0 || width > MAX_THUMBNAIL_SIDE || height > MAX_THUMBNAIL_SIDE {
        return Err("Invalid schematic thumbnail dimensions".into());
    }
    let rgba = reader()
        .decode()
        .map_err(|e| e.to_string())?
        .into_rgba8()
        .into_raw()
        .into();
    Ok(Thumbnail {
        width,
        height,
        rgba,
    })
}
