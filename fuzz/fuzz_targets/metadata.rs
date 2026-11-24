//! Arbitrary bytes given to the metadata code: reading the blocks of a
//! container, editing an EXIF block and writing the blocks back.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pixelit_codecs::metadata::{self, Metadata};
use pixelit_core::ImageFormat;

/// Containers PixelIt reads metadata from and writes it into.
const FORMATS: &[ImageFormat] = &[
    ImageFormat::Jpeg,
    ImageFormat::Png,
    ImageFormat::Webp,
    ImageFormat::Avif,
    ImageFormat::Jxl,
    ImageFormat::Tiff,
];

fuzz_target!(|data: &[u8]| {
    let Some((selector, bytes)) = data.split_first() else {
        return;
    };
    let format = FORMATS[usize::from(*selector) % FORMATS.len()];

    // The input as a container.
    let found = metadata::extract(bytes, format);
    let tags = [String::from("fuzz"), String::from("<tag> & \"quotes\"")];
    let caption = metadata::caption_xmp(found.xmp.as_deref(), "A <caption> & more", &tags);
    if let Some(xmp) = &caption {
        let _ = metadata::replace_xmp(bytes, format, xmp);
    }
    let _ = metadata::embed(bytes.to_vec(), format, &found);

    // The input as an EXIF block.
    let _ = metadata::orientation(bytes);
    let mut exif = bytes.to_vec();
    metadata::reset_orientation(&mut exif);
    metadata::strip_gps(&mut exif);
    metadata::strip_thumbnail(&mut exif);
    let exif = metadata::normalize_exif(exif);

    // The input as the blocks written into a new file.
    let blocks = Metadata {
        exif,
        icc: Some(bytes.to_vec()),
        xmp: Some(bytes.to_vec()),
    };
    let _ = metadata::embed(bytes.to_vec(), format, &blocks);
});
