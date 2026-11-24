//! EXIF, ICC and XMP blocks: the metadata policy and embedding into files.

use std::io::Cursor;

use exif::experimental::Writer;
use exif::{Field, In, Reader, Tag, Value};
use img_parts::jpeg::{markers, Jpeg, JpegSegment};
use img_parts::png::{Png, PngChunk};
use img_parts::webp::WebP;
use img_parts::{Bytes, ImageEXIF, ImageICC};
use pixelit_core::pipeline::{ColorPolicy, MetadataMode, MetadataPolicy};
use pixelit_core::ImageFormat;

const EXIF_PREFIX: &[u8] = b"Exif\0\0";
const XMP_JPEG_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_PNG_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
/// A JPEG segment holds at most 65 533 bytes of payload.
const JPEG_SEGMENT_MAX: usize = 65_533;

const TAG_ORIENTATION: u16 = 0x0112;
const TAG_GPS_IFD: u16 = 0x8825;
const TAG_THUMBNAIL_OFFSET: u16 = 0x0201;
const TAG_THUMBNAIL_LENGTH: u16 = 0x0202;
const ENTRY_SIZE: usize = 12;

/// Metadata blocks of a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    /// EXIF as a TIFF structure, without the `Exif\0\0` prefix.
    pub exif: Option<Vec<u8>>,
    /// ICC colour profile.
    pub icc: Option<Vec<u8>>,
    /// XMP packet.
    pub xmp: Option<Vec<u8>>,
}

impl Metadata {
    pub fn is_empty(&self) -> bool {
        self.exif.is_none() && self.icc.is_none() && self.xmp.is_none()
    }
}

/// Removes the `Exif\0\0` prefix that some containers keep in front of the TIFF data.
pub fn normalize_exif(mut exif: Vec<u8>) -> Option<Vec<u8>> {
    if exif.starts_with(EXIF_PREFIX) {
        exif.drain(..EXIF_PREFIX.len());
    }
    Tiff::open(&exif).is_some().then_some(exif)
}

/// Read access to the TIFF structure of an EXIF block.
struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}

/// One directory entry.
struct Entry {
    /// Offset of the entry inside the block.
    at: usize,
    tag: u16,
    /// Bytes taken by the value.
    size: usize,
    /// The value field read as an offset.
    value: u32,
}

impl<'a> Tiff<'a> {
    fn open(data: &'a [u8]) -> Option<Self> {
        let little = match data.get(..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Self { data, little })
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let bytes: [u8; 2] = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let bytes: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn first_ifd(&self) -> Option<usize> {
        self.u32_at(4).map(|offset| offset as usize)
    }

    /// Entries of the directory at `ifd` and the position of its "next" pointer.
    fn entries(&self, ifd: usize) -> Option<(Vec<Entry>, usize)> {
        let count = usize::from(self.u16_at(ifd)?);
        let end = ifd.checked_add(2 + count * ENTRY_SIZE)?;
        if end + 4 > self.data.len() {
            return None;
        }
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            let at = ifd + 2 + index * ENTRY_SIZE;
            let kind = self.u16_at(at + 2)?;
            let unit: usize = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => 0,
            };
            let count = self.u32_at(at + 4)? as usize;
            entries.push(Entry {
                at,
                tag: self.u16_at(at)?,
                size: unit.checked_mul(count)?,
                value: self.u32_at(at + 8)?,
            });
        }
        Some((entries, end))
    }
}

fn put_u16(data: &mut [u8], at: usize, value: u16, little: bool) {
    let bytes = if little {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    data[at..at + 2].copy_from_slice(&bytes);
}

fn zero(data: &mut [u8], start: usize, len: usize) {
    let end = start.saturating_add(len).min(data.len());
    if start < end {
        data[start..end].fill(0);
    }
}

/// The EXIF orientation (1 to 8) stored in the block.
pub fn orientation(exif: &[u8]) -> Option<u16> {
    let tiff = Tiff::open(exif)?;
    let (entries, _) = tiff.entries(tiff.first_ifd()?)?;
    let entry = entries.iter().find(|entry| entry.tag == TAG_ORIENTATION)?;
    let value = tiff.u16_at(entry.at + 8)?;
    (1..=8).contains(&value).then_some(value)
}

/// Sets the orientation to "upright" because the pixels are stored rotated already.
pub fn reset_orientation(exif: &mut [u8]) {
    let found = Tiff::open(exif).and_then(|tiff| {
        let (entries, _) = tiff.entries(tiff.first_ifd()?)?;
        let entry = entries.iter().find(|entry| entry.tag == TAG_ORIENTATION)?;
        Some((entry.at + 8, tiff.little))
    });
    if let Some((at, little)) = found {
        put_u16(exif, at, 1, little);
    }
}

/// Removes one entry from a directory and returns the entry it removed.
fn remove_entry(exif: &mut [u8], tag: u16) -> Option<Entry> {
    let (entry, ifd, count, end, little) = {
        let tiff = Tiff::open(exif)?;
        let ifd = tiff.first_ifd()?;
        let (mut entries, end) = tiff.entries(ifd)?;
        let index = entries.iter().position(|entry| entry.tag == tag)?;
        let count = entries.len();
        (entries.swap_remove(index), ifd, count, end, tiff.little)
    };
    // Entries after the removed one and the "next" pointer move up by one slot.
    exif.copy_within(entry.at + ENTRY_SIZE..end + 4, entry.at);
    zero(exif, end + 4 - ENTRY_SIZE, ENTRY_SIZE);
    put_u16(exif, ifd, (count - 1) as u16, little);
    Some(entry)
}

/// Removes the GPS directory and wipes the coordinates it pointed to.
pub fn strip_gps(exif: &mut [u8]) {
    let Some(pointer) = remove_entry(exif, TAG_GPS_IFD) else {
        return;
    };
    let gps = pointer.value as usize;
    let wipe: Option<(Vec<(usize, usize)>, usize)> = Tiff::open(exif).and_then(|tiff| {
        let (entries, end) = tiff.entries(gps)?;
        let ranges = entries
            .iter()
            .filter(|entry| entry.size > 4)
            .map(|entry| (entry.value as usize, entry.size))
            .collect();
        Some((ranges, end + 4 - gps))
    });
    if let Some((ranges, length)) = wipe {
        for (start, len) in ranges {
            zero(exif, start, len);
        }
        zero(exif, gps, length);
    }
}

/// Removes the embedded thumbnail: after editing it shows the old picture.
pub fn strip_thumbnail(exif: &mut [u8]) {
    let found = Tiff::open(exif).and_then(|tiff| {
        let (_, next_at) = tiff.entries(tiff.first_ifd()?)?;
        let second = tiff.u32_at(next_at)? as usize;
        if second == 0 {
            return None;
        }
        let mut wipe = Vec::new();
        if let Some((entries, end)) = tiff.entries(second) {
            let value = |tag: u16| {
                entries
                    .iter()
                    .find(|entry| entry.tag == tag)
                    .map(|entry| entry.value as usize)
            };
            if let (Some(offset), Some(length)) =
                (value(TAG_THUMBNAIL_OFFSET), value(TAG_THUMBNAIL_LENGTH))
            {
                wipe.push((offset, length));
            }
            for entry in entries.iter().filter(|entry| entry.size > 4) {
                wipe.push((entry.value as usize, entry.size));
            }
            wipe.push((second, end + 4 - second));
        }
        Some((next_at, wipe))
    });
    if let Some((next_at, wipe)) = found {
        for (start, len) in wipe {
            zero(exif, start, len);
        }
        zero(exif, next_at, 4);
    }
}

fn ascii(text: &str) -> Value {
    Value::Ascii(vec![text.as_bytes().to_vec()])
}

/// Writes the author and copyright fields, keeping the other primary fields.
fn with_credits(exif: Option<&[u8]>, author: &str, copyright: &str) -> Option<Vec<u8>> {
    let parsed = exif.and_then(|bytes| Reader::new().read_raw(bytes.to_vec()).ok());
    let little = parsed.as_ref().is_none_or(exif::Exif::little_endian);
    let replaced = |tag: Tag| {
        (tag == Tag::Artist && !author.is_empty())
            || (tag == Tag::Copyright && !copyright.is_empty())
    };
    let mut fields: Vec<Field> = parsed.as_ref().map_or_default(|parsed| {
        parsed
            .fields()
            .filter(|field| field.ifd_num == In::PRIMARY && !replaced(field.tag))
            .cloned()
            .collect()
    });
    if !author.is_empty() {
        fields.push(Field {
            tag: Tag::Artist,
            ifd_num: In::PRIMARY,
            value: ascii(author),
        });
    }
    if !copyright.is_empty() {
        fields.push(Field {
            tag: Tag::Copyright,
            ifd_num: In::PRIMARY,
            value: ascii(copyright),
        });
    }
    let mut writer = Writer::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut out = Cursor::new(Vec::new());
    writer.write(&mut out, little).ok()?;
    Some(out.into_inner())
}

/// Whether the format can carry an ICC profile written by this crate.
pub fn supports_icc(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Jpeg
            | ImageFormat::Png
            | ImageFormat::Webp
            | ImageFormat::Jxl
            | ImageFormat::Tiff
    )
}

/// Whether the format can carry an EXIF block written by this crate.
pub fn supports_exif(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Jpeg
            | ImageFormat::Png
            | ImageFormat::Webp
            | ImageFormat::Avif
            | ImageFormat::Jxl
    )
}

/// Whether the format can carry an XMP packet written by this crate.
pub fn supports_xmp(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Jxl
    )
}

/// The metadata to write into an output file.
///
/// `source` is the metadata of the input. The result keeps only what the
/// policy allows and what the target format can store. An ICC profile in the
/// result tells the encoder to convert the pixels into that profile.
pub fn prepare(
    source: &Metadata,
    policy: &MetadataPolicy,
    color: ColorPolicy,
    format: ImageFormat,
) -> Metadata {
    let (keep_exif, keep_gps, keep_xmp, keep_icc) = match policy.mode {
        MetadataMode::Keep => (true, true, true, true),
        MetadataMode::Strip => (false, false, false, false),
        MetadataMode::Custom => (
            policy.keep_exif,
            policy.keep_gps,
            policy.keep_xmp,
            policy.keep_icc,
        ),
    };
    let custom = policy.mode == MetadataMode::Custom;
    let author = if custom { policy.author.trim() } else { "" };
    let copyright = if custom { policy.copyright.trim() } else { "" };

    let mut exif = source
        .exif
        .clone()
        .filter(|_| keep_exif && supports_exif(format));
    if let Some(block) = exif.as_mut() {
        reset_orientation(block);
        strip_thumbnail(block);
        if !keep_gps {
            strip_gps(block);
        }
    }
    if supports_exif(format) && (!author.is_empty() || !copyright.is_empty()) {
        if let Some(block) = with_credits(exif.as_deref(), author, copyright) {
            exif = Some(block);
        }
    }

    let icc = source.icc.clone().filter(|profile| {
        keep_icc
            && color == ColorPolicy::Original
            && supports_icc(format)
            && crate::color::needs_conversion(profile)
    });
    let xmp = source
        .xmp
        .clone()
        .filter(|_| keep_xmp && supports_xmp(format));
    Metadata { exif, icc, xmp }
}

fn embed_jpeg(bytes: Vec<u8>, metadata: &Metadata) -> Option<Vec<u8>> {
    let mut jpeg = Jpeg::from_bytes(Bytes::from(bytes)).ok()?;
    if let Some(icc) = &metadata.icc {
        jpeg.set_icc_profile(Some(Bytes::from(icc.clone())));
    }
    if let Some(exif) = &metadata.exif {
        jpeg.set_exif(Some(Bytes::from(exif.clone())));
    }
    if let Some(xmp) = &metadata.xmp {
        if XMP_JPEG_PREFIX.len() + xmp.len() <= JPEG_SEGMENT_MAX {
            let mut contents = XMP_JPEG_PREFIX.to_vec();
            contents.extend_from_slice(xmp);
            let segments = jpeg.segments_mut();
            let at = segments
                .iter()
                .position(|segment| !matches!(segment.marker(), markers::APP0 | markers::APP1))
                .unwrap_or(segments.len());
            segments.insert(
                at,
                JpegSegment::new_with_contents(markers::APP1, Bytes::from(contents)),
            );
        }
    }
    Some(jpeg.encoder().bytes().to_vec())
}

fn embed_png(bytes: Vec<u8>, metadata: &Metadata) -> Option<Vec<u8>> {
    let mut png = Png::from_bytes(Bytes::from(bytes)).ok()?;
    if let Some(icc) = &metadata.icc {
        png.set_icc_profile(Some(Bytes::from(icc.clone())));
    }
    if let Some(exif) = &metadata.exif {
        png.set_exif(Some(Bytes::from(exif.clone())));
    }
    if let Some(xmp) = &metadata.xmp {
        // iTXt: keyword, 0, compression flag, compression method, language, 0,
        // translated keyword, 0, text.
        let mut contents = XMP_PNG_KEYWORD.to_vec();
        contents.extend_from_slice(&[0, 0, 0, 0, 0]);
        contents.extend_from_slice(xmp);
        let chunks = png.chunks_mut();
        let at = chunks
            .iter()
            .position(|chunk| chunk.kind() == *b"IDAT")
            .unwrap_or(chunks.len());
        chunks.insert(at, PngChunk::new(*b"iTXt", Bytes::from(contents)));
    }
    Some(png.encoder().bytes().to_vec())
}

fn embed_webp(bytes: Vec<u8>, metadata: &Metadata) -> Option<Vec<u8>> {
    let mut webp = WebP::from_bytes(Bytes::from(bytes)).ok()?;
    if let Some(icc) = &metadata.icc {
        webp.set_icc_profile(Some(Bytes::from(icc.clone())));
    }
    if let Some(exif) = &metadata.exif {
        webp.set_exif(Some(Bytes::from(exif.clone())));
    }
    Some(webp.encoder().bytes().to_vec())
}

/// Writes the metadata into an encoded JPEG, PNG or WebP file. Other formats
/// and files the container parser rejects come back unchanged.
pub fn embed(bytes: Vec<u8>, format: ImageFormat, metadata: &Metadata) -> Vec<u8> {
    if metadata.is_empty() {
        return bytes;
    }
    let written = match format {
        ImageFormat::Jpeg => embed_jpeg(bytes.clone(), metadata),
        ImageFormat::Png => embed_png(bytes.clone(), metadata),
        ImageFormat::Webp => embed_webp(bytes.clone(), metadata),
        _ => None,
    };
    written.unwrap_or(bytes)
}

/// Reads the metadata blocks of an encoded JPEG, PNG or WebP file.
pub fn extract(bytes: &[u8], format: ImageFormat) -> Metadata {
    let shared = Bytes::copy_from_slice(bytes);
    let (icc, exif, xmp) = match format {
        ImageFormat::Jpeg => match Jpeg::from_bytes(shared) {
            Ok(jpeg) => {
                let xmp = jpeg
                    .segments_by_marker(markers::APP1)
                    .find(|segment| segment.contents().starts_with(XMP_JPEG_PREFIX))
                    .map(|segment| segment.contents()[XMP_JPEG_PREFIX.len()..].to_vec());
                (jpeg.icc_profile(), jpeg.exif(), xmp)
            }
            Err(_) => (None, None, None),
        },
        ImageFormat::Png => match Png::from_bytes(shared) {
            Ok(png) => {
                let xmp = png
                    .chunks_by_type(*b"iTXt")
                    .find(|chunk| chunk.contents().starts_with(XMP_PNG_KEYWORD))
                    .and_then(|chunk| {
                        let body = chunk.contents().get(XMP_PNG_KEYWORD.len() + 3..)?;
                        // Skip the language tag and the translated keyword.
                        let language = body.iter().position(|&byte| byte == 0)?;
                        let rest = body.get(language + 1..)?;
                        let keyword = rest.iter().position(|&byte| byte == 0)?;
                        Some(rest.get(keyword + 1..)?.to_vec())
                    });
                (png.icc_profile(), png.exif(), xmp)
            }
            Err(_) => (None, None, None),
        },
        ImageFormat::Webp => match WebP::from_bytes(shared) {
            Ok(webp) => {
                let xmp = webp
                    .chunk_by_id(img_parts::webp::CHUNK_XMP)
                    .and_then(|chunk| chunk.content().data())
                    .map(|data| data.to_vec());
                (webp.icc_profile(), webp.exif(), xmp)
            }
            Err(_) => (None, None, None),
        },
        _ => (None, None, None),
    };
    Metadata {
        exif: exif.and_then(|block| normalize_exif(block.to_vec())),
        icc: icc
            .map(|block| block.to_vec())
            .filter(|block| !block.is_empty()),
        xmp,
    }
}

/// Start of the block PixelIt adds to an XMP packet; the attribute marks the
/// block so a later caption replaces it.
const CAPTION_OPEN: &str = "<rdf:Description rdf:about=\"\" xmlns:pixelit=\"https://github.com/Crazymoralfreak/PixelIt/ns/1.0/\" pixelit:caption=\"1\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">";
const CAPTION_CLOSE: &str = "</rdf:Description>";
const RDF_CLOSE: &str = "</rdf:RDF>";

fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            // Control characters are not allowed in XML 1.0.
            other if other.is_control() && !matches!(other, '\n' | '\t') => escaped.push(' '),
            other => escaped.push(other),
        }
    }
    escaped
}

/// An XMP packet carrying a description (`dc:description`) and keywords
/// (`dc:subject`). The rest of an existing packet is kept; `None` means the
/// existing packet has a form this crate cannot extend.
pub fn caption_xmp(existing: Option<&[u8]>, description: &str, tags: &[String]) -> Option<Vec<u8>> {
    let mut block = String::from(CAPTION_OPEN);
    if !description.trim().is_empty() {
        block.push_str("<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">");
        block.push_str(&escape_xml(description.trim()));
        block.push_str("</rdf:li></rdf:Alt></dc:description>");
    }
    if !tags.is_empty() {
        block.push_str("<dc:subject><rdf:Bag>");
        for tag in tags {
            block.push_str("<rdf:li>");
            block.push_str(&escape_xml(tag.trim()));
            block.push_str("</rdf:li>");
        }
        block.push_str("</rdf:Bag></dc:subject>");
    }
    block.push_str(CAPTION_CLOSE);
    let Some(existing) = existing.filter(|packet| !packet.is_empty()) else {
        return Some(
            format!(
                "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{block}</rdf:RDF>\
</x:xmpmeta><?xpacket end=\"w\"?>"
            )
            .into_bytes(),
        );
    };
    let mut packet = std::str::from_utf8(existing).ok()?.to_owned();
    if let Some(start) = packet.find(CAPTION_OPEN) {
        let length = packet[start..].find(CAPTION_CLOSE)?;
        packet.replace_range(start..start + length + CAPTION_CLOSE.len(), "");
    }
    let at = packet.rfind(RDF_CLOSE)?;
    packet.insert_str(at, &block);
    Some(packet.into_bytes())
}

/// Puts an XMP packet into an encoded JPEG or PNG file in place of the one it
/// has. The pixels are not touched. `None` means the file cannot take it.
pub fn replace_xmp(bytes: &[u8], format: ImageFormat, xmp: &[u8]) -> Option<Vec<u8>> {
    let shared = Bytes::copy_from_slice(bytes);
    match format {
        ImageFormat::Jpeg => {
            if XMP_JPEG_PREFIX.len() + xmp.len() > JPEG_SEGMENT_MAX {
                return None;
            }
            let mut jpeg = Jpeg::from_bytes(shared).ok()?;
            let segments = jpeg.segments_mut();
            segments.retain(|segment| {
                !(segment.marker() == markers::APP1
                    && segment.contents().starts_with(XMP_JPEG_PREFIX))
            });
            let mut contents = XMP_JPEG_PREFIX.to_vec();
            contents.extend_from_slice(xmp);
            let at = segments
                .iter()
                .position(|segment| !matches!(segment.marker(), markers::APP0 | markers::APP1))
                .unwrap_or(segments.len());
            segments.insert(
                at,
                JpegSegment::new_with_contents(markers::APP1, Bytes::from(contents)),
            );
            Some(jpeg.encoder().bytes().to_vec())
        }
        ImageFormat::Png => {
            let mut png = Png::from_bytes(shared).ok()?;
            let chunks = png.chunks_mut();
            chunks.retain(|chunk| {
                !(chunk.kind() == *b"iTXt" && chunk.contents().starts_with(XMP_PNG_KEYWORD))
            });
            let mut contents = XMP_PNG_KEYWORD.to_vec();
            contents.extend_from_slice(&[0, 0, 0, 0, 0]);
            contents.extend_from_slice(xmp);
            let at = chunks
                .iter()
                .position(|chunk| chunk.kind() == *b"IDAT")
                .unwrap_or(chunks.len());
            chunks.insert(at, PngChunk::new(*b"iTXt", Bytes::from(contents)));
            Some(png.encoder().bytes().to_vec())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_are_written_and_replaced() {
        let tags = vec![String::from("fox"), String::from("snow & ice")];
        let first = caption_xmp(None, "A <red> fox", &tags).unwrap();
        let text = String::from_utf8(first.clone()).unwrap();
        assert!(text.contains("A &lt;red&gt; fox"));
        assert!(text.contains("<rdf:li>snow &amp; ice</rdf:li>"));
        assert!(text.ends_with("<?xpacket end=\"w\"?>"));

        let second = caption_xmp(Some(&first), "A wolf", &[]).unwrap();
        let text = String::from_utf8(second).unwrap();
        assert!(text.contains("A wolf"));
        assert!(!text.contains("fox"));
        assert_eq!(text.matches(CAPTION_OPEN).count(), 1);
        assert_eq!(text.matches(RDF_CLOSE).count(), 1);

        assert!(caption_xmp(Some(b"not a packet"), "x", &[]).is_none());
        assert!(caption_xmp(Some(&[0xff, 0xfe, 0x00]), "x", &[]).is_none());
    }

    #[test]
    fn xmp_is_replaced_in_png_files() {
        let image =
            pixelit_core::Image::new(4, 4, pixelit_core::Depth::Eight, pixelit_core::Color::WHITE)
                .unwrap();
        let png = crate::encode(
            &image,
            &crate::EncodeOptions::new(ImageFormat::Png),
            &pixelit_core::CancelToken::new(),
        )
        .unwrap();
        let first = replace_xmp(&png, ImageFormat::Png, b"<one/>").unwrap();
        assert_eq!(
            extract(&first, ImageFormat::Png).xmp.as_deref(),
            Some(&b"<one/>"[..])
        );
        let second = replace_xmp(&first, ImageFormat::Png, b"<two/>").unwrap();
        assert_eq!(
            extract(&second, ImageFormat::Png).xmp.as_deref(),
            Some(&b"<two/>"[..])
        );
        assert!(crate::decode_bytes(
            &second,
            None,
            &crate::DecodeOptions::default(),
            &pixelit_core::CancelToken::new()
        )
        .is_ok());
        assert!(replace_xmp(&png, ImageFormat::Bmp, b"<x/>").is_none());
        assert!(replace_xmp(b"garbage", ImageFormat::Jpeg, b"<x/>").is_none());
    }

    /// A little-endian EXIF block with orientation 6, a GPS directory holding
    /// one rational value and a second directory with a 4-byte thumbnail.
    fn sample() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"II*\0");
        data.extend_from_slice(&8_u32.to_le_bytes());
        // IFD0 at 8: 2 entries.
        data.extend_from_slice(&2_u16.to_le_bytes());
        // Orientation, SHORT, count 1, value 6.
        data.extend_from_slice(&0x0112_u16.to_le_bytes());
        data.extend_from_slice(&3_u16.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        data.extend_from_slice(&6_u32.to_le_bytes());
        // GPS pointer, LONG, count 1, value 38.
        data.extend_from_slice(&0x8825_u16.to_le_bytes());
        data.extend_from_slice(&4_u16.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        data.extend_from_slice(&38_u32.to_le_bytes());
        // Next IFD at 64.
        data.extend_from_slice(&64_u32.to_le_bytes());
        assert_eq!(data.len(), 38);
        // GPS IFD at 38: 1 entry, RATIONAL x1 stored at 56.
        data.extend_from_slice(&1_u16.to_le_bytes());
        data.extend_from_slice(&0x0002_u16.to_le_bytes());
        data.extend_from_slice(&5_u16.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        data.extend_from_slice(&56_u32.to_le_bytes());
        data.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(data.len(), 56);
        data.extend_from_slice(&[0xAA; 8]);
        assert_eq!(data.len(), 64);
        // IFD1 at 64: thumbnail offset 94 and length 4.
        data.extend_from_slice(&2_u16.to_le_bytes());
        data.extend_from_slice(&0x0201_u16.to_le_bytes());
        data.extend_from_slice(&4_u16.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        data.extend_from_slice(&94_u32.to_le_bytes());
        data.extend_from_slice(&0x0202_u16.to_le_bytes());
        data.extend_from_slice(&4_u16.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        data.extend_from_slice(&4_u32.to_le_bytes());
        data.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(data.len(), 94);
        data.extend_from_slice(&[0xBB; 4]);
        data
    }

    #[test]
    fn reads_and_resets_orientation() {
        let mut exif = sample();
        assert_eq!(orientation(&exif), Some(6));
        reset_orientation(&mut exif);
        assert_eq!(orientation(&exif), Some(1));
    }

    #[test]
    fn strips_gps_and_wipes_coordinates() {
        let mut exif = sample();
        strip_gps(&mut exif);
        assert_eq!(&exif[8..10], &1_u16.to_le_bytes());
        // Orientation is still readable and the thumbnail pointer moved up.
        assert_eq!(orientation(&exif), Some(6));
        assert_eq!(&exif[22..26], &64_u32.to_le_bytes());
        assert!(exif[38..64].iter().all(|&byte| byte == 0));
        assert!(!exif.contains(&0xAA));
    }

    #[test]
    fn strips_thumbnail() {
        let mut exif = sample();
        strip_thumbnail(&mut exif);
        assert_eq!(&exif[34..38], &[0, 0, 0, 0]);
        assert!(!exif.contains(&0xBB));
        assert_eq!(orientation(&exif), Some(6));
    }

    #[test]
    fn normalizes_prefix() {
        let mut prefixed = EXIF_PREFIX.to_vec();
        prefixed.extend_from_slice(&sample());
        assert_eq!(normalize_exif(prefixed), Some(sample()));
        assert_eq!(normalize_exif(b"junk".to_vec()), None);
    }

    #[test]
    fn broken_blocks_do_not_panic() {
        for cut in 0..sample().len() {
            let mut exif = sample()[..cut].to_vec();
            let _ = orientation(&exif);
            reset_orientation(&mut exif);
            strip_gps(&mut exif);
            strip_thumbnail(&mut exif);
        }
    }

    #[test]
    fn policy_strip_removes_everything() {
        let source = Metadata {
            exif: Some(sample()),
            icc: Some(vec![1, 2, 3]),
            xmp: Some(b"<x/>".to_vec()),
        };
        let policy = MetadataPolicy {
            mode: MetadataMode::Strip,
            ..MetadataPolicy::default()
        };
        let result = prepare(&source, &policy, ColorPolicy::Srgb, ImageFormat::Jpeg);
        assert!(result.is_empty());
    }

    #[test]
    fn policy_keep_resets_orientation_and_thumbnail() {
        let source = Metadata {
            exif: Some(sample()),
            icc: None,
            xmp: Some(b"<x/>".to_vec()),
        };
        let policy = MetadataPolicy::default();
        let result = prepare(&source, &policy, ColorPolicy::Srgb, ImageFormat::Jpeg);
        let exif = result.exif.unwrap();
        assert_eq!(orientation(&exif), Some(1));
        assert!(!exif.contains(&0xBB));
        // Keep mode leaves the GPS data in place.
        assert!(exif.contains(&0xAA));
        assert_eq!(result.xmp.as_deref(), Some(&b"<x/>"[..]));
    }

    #[test]
    fn custom_policy_writes_credits_without_gps() {
        let source = Metadata {
            exif: Some(sample()),
            icc: None,
            xmp: None,
        };
        let policy = MetadataPolicy {
            mode: MetadataMode::Custom,
            keep_exif: true,
            keep_gps: false,
            copyright: "(c) Studio".into(),
            author: "Mityy".into(),
            ..MetadataPolicy::default()
        };
        let result = prepare(&source, &policy, ColorPolicy::Srgb, ImageFormat::Jpeg);
        let exif = result.exif.unwrap();
        let parsed = Reader::new().read_raw(exif.clone()).unwrap();
        let text = |tag: Tag| {
            parsed
                .get_field(tag, In::PRIMARY)
                .map(|field| field.display_value().to_string())
        };
        assert_eq!(text(Tag::Artist).as_deref(), Some("\"Mityy\""));
        assert_eq!(text(Tag::Copyright).as_deref(), Some("\"(c) Studio\""));
        assert!(parsed.get_field(Tag::GPSLatitude, In::PRIMARY).is_none());
        assert!(!exif.contains(&0xAA));
    }

    #[test]
    fn formats_without_exif_drop_it() {
        let source = Metadata {
            exif: Some(sample()),
            icc: None,
            xmp: None,
        };
        let result = prepare(
            &source,
            &MetadataPolicy::default(),
            ColorPolicy::Srgb,
            ImageFormat::Bmp,
        );
        assert!(result.is_empty());
    }
}
