//! Arbitrary bytes given to the decoders the way the application gives them
//! a file: with the format named by the caller or found by the first bytes.

#![no_main]

use std::panic;
use std::sync::Once;

use libfuzzer_sys::fuzz_target;
use pixelit_codecs::decode::detect;
use pixelit_codecs::{decode_bytes, probe_bytes, DecodeOptions};
use pixelit_core::{CancelToken, ImageFormat, Limits};

/// Formats read without a system component. PDF needs PDFium and HEIC needs
/// Windows, so they are left out. AVIF is left out as well: its AV1 decoder
/// ends the process on some damaged files, which is why the application
/// decodes AVIF in a helper process; here every run would end on that.
const FORMATS: &[ImageFormat] = &[
    ImageFormat::Jpeg,
    ImageFormat::Png,
    ImageFormat::Webp,
    ImageFormat::Jxl,
    ImageFormat::Tiff,
    ImageFormat::Bmp,
    ImageFormat::Gif,
    ImageFormat::Ico,
    ImageFormat::Raw,
    ImageFormat::Svg,
    ImageFormat::Psd,
];

/// A damaged header may claim any size; these limits keep one run small.
const LIMITS: Limits = Limits {
    max_side: 2048,
    max_pixels: 1_000_000,
    max_file_bytes: 1024 * 1024,
    max_animation_pixels: 4_000_000,
};

static QUIET: Once = Once::new();

fuzz_target!(|data: &[u8]| {
    // PixelIt turns a panic inside a third-party decoder into an error, and
    // the fuzzer's own panic hook would stop the process before that happens.
    // Without the hook a caught panic passes; a panic that escapes the
    // decoding functions still ends the run as a finding.
    QUIET.call_once(|| panic::set_hook(Box::new(|_| {})));

    let Some((selector, bytes)) = data.split_first() else {
        return;
    };
    // One value more than there are formats: the format is found by the content.
    let hint = FORMATS
        .get(usize::from(*selector) % (FORMATS.len() + 1))
        .copied();
    if hint.is_none() && detect(bytes, None) == Some(ImageFormat::Avif) {
        return;
    }
    let options = DecodeOptions {
        limits: LIMITS,
        ..DecodeOptions::default()
    };
    if let Ok(decoded) = decode_bytes(bytes, hint, &options, &CancelToken::new()) {
        let image = decoded.media.first().expect("a decoded file has a frame");
        assert!(
            LIMITS.check_size(image.width(), image.height()).is_ok(),
            "an image larger than the limits was accepted"
        );
    }
    let _ = probe_bytes(bytes, hint);
});
