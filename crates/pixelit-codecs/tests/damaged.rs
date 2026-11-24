//! Damaged files. Every format PixelIt writes is encoded here and then cut
//! short, overwritten and padded in a fixed set of ways. Reading such a file
//! must end with an answer (an image or an error), never with a crash, and an
//! image that does come out must respect the size limits.

use std::path::PathBuf;

use pixelit_codecs::decode::set_avif_helper;
use pixelit_codecs::metadata;
use pixelit_codecs::{decode_bytes, encode, encode_media, probe_bytes};
use pixelit_codecs::{DecodeOptions, EncodeOptions};
use pixelit_core::image::{Animation, Frame};
use pixelit_core::{CancelToken, Color, Depth, Image, ImageFormat, Limits, Media};

const WIDTH: u32 = 40;
const HEIGHT: u32 = 28;

/// A damaged header may claim any size; these limits keep the test small.
const LIMITS: Limits = Limits {
    max_side: 4096,
    max_pixels: 4_000_000,
    max_file_bytes: 16 * 1024 * 1024,
    max_animation_pixels: 16_000_000,
};

/// A gradient with a transparent corner.
fn sample() -> Image {
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let opacity = if x < WIDTH / 4 && y < HEIGHT / 4 {
                0
            } else {
                255
            };
            data.extend_from_slice(&[
                (x * 255 / WIDTH) as u8,
                (y * 255 / HEIGHT) as u8,
                ((x + y) * 255 / (WIDTH + HEIGHT)) as u8,
                opacity,
            ]);
        }
    }
    Image::from_rgba8(WIDTH, HEIGHT, data).expect("matching buffer")
}

/// The same sequence of numbers on every run.
struct Sequence(u64);

impl Sequence {
    fn step(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.step() % limit.max(1) as u64) as usize
    }
}

/// Intact files: one of every format PixelIt writes, an animation and an SVG.
fn intact_files() -> Vec<(ImageFormat, Vec<u8>)> {
    let cancel = CancelToken::new();
    let image = sample();
    let mut files: Vec<(ImageFormat, Vec<u8>)> = ImageFormat::OUTPUT
        .iter()
        .map(|format| {
            let bytes = encode(&image, &EncodeOptions::new(*format), &cancel)
                .unwrap_or_else(|error| panic!("{format:?} was not written: {error:?}"));
            (*format, bytes)
        })
        .collect();

    let frames = [
        Color::rgb(200, 30, 30),
        Color::rgb(30, 200, 30),
        Color::rgb(30, 30, 200),
    ]
    .into_iter()
    .map(|colour| Frame {
        image: Image::new(WIDTH, HEIGHT, Depth::Eight, colour).expect("a small frame"),
        delay_ms: 80,
    })
    .collect();
    let animation = Media::Animated(Animation {
        frames,
        loop_count: 0,
    });
    let gif = encode_media(&animation, &EncodeOptions::new(ImageFormat::Gif), &cancel)
        .expect("an animated GIF");
    files.push((ImageFormat::Gif, gif));

    let svg =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="28" viewBox="0 0 40 28">
  <rect width="40" height="28" fill="#2f6fed"/>
  <circle cx="20" cy="14" r="9" fill="#ffffff" opacity="0.8"/>
  <path d="M4 24 L16 8 L28 24 Z" fill="none" stroke="#111111" stroke-width="2"/>
</svg>"##;
    files.push((ImageFormat::Svg, svg.to_vec()));
    files
}

/// Ways a file gets damaged: a download that stopped, a bad sector, a wrong
/// byte in the header, junk after the end.
fn damaged_copies(bytes: &[u8], seed: u64) -> Vec<(String, Vec<u8>)> {
    let length = bytes.len();
    let mut copies = Vec::new();

    let mut cuts: Vec<usize> = vec![0, 1, 2, 3, 4, 8, 12, 16, 24, 32, 48, 64, 96, 128];
    cuts.extend((1..16).map(|part| length * part / 16));
    cuts.extend([length.saturating_sub(8), length.saturating_sub(1)]);
    cuts.retain(|cut| *cut < length);
    cuts.sort_unstable();
    cuts.dedup();
    for cut in cuts {
        copies.push((format!("cut at {cut}"), bytes[..cut].to_vec()));
    }

    // Every byte of the header in turn.
    for index in 0..length.min(48) {
        for value in [0x00, 0xFF] {
            if bytes[index] != value {
                let mut copy = bytes.to_vec();
                copy[index] = value;
                copies.push((format!("byte {index} set to {value:#04x}"), copy));
            }
        }
    }

    let mut sequence = Sequence(seed | 1);
    for _ in 0..96 {
        let index = sequence.below(length);
        let value = sequence.below(256) as u8;
        let mut copy = bytes.to_vec();
        copy[index] = value;
        copies.push((format!("byte {index} set to {value:#04x}"), copy));
    }

    for _ in 0..8 {
        let start = sequence.below(length);
        let end = (start + 1 + sequence.below(32)).min(length);
        let mut zeroed = bytes.to_vec();
        zeroed[start..end].fill(0);
        copies.push((format!("bytes {start}..{end} zeroed"), zeroed));
        let mut removed = bytes.to_vec();
        removed.drain(start..end);
        copies.push((format!("bytes {start}..{end} removed"), removed));
    }

    let mut padded = bytes.to_vec();
    padded.extend((0..64).map(|_| sequence.below(256) as u8));
    copies.push((String::from("junk after the end"), padded));

    let mut doubled = bytes.to_vec();
    doubled.extend_from_slice(bytes);
    copies.push((String::from("the file written twice"), doubled));

    copies
}

fn within_limits(image: &Image) -> bool {
    LIMITS.check_size(image.width(), image.height()).is_ok()
}

/// The AV1 decoder ends its process on some damaged files, so the application
/// decodes AVIF in a helper process. The tests do the same.
fn use_the_avif_helper() {
    set_avif_helper(PathBuf::from(env!("CARGO_BIN_EXE_pixelit-decode")));
}

/// Reads one damaged copy every way the application does.
fn read(format: ImageFormat, bytes: &[u8], what: &str) {
    let cancel = CancelToken::new();
    let options = DecodeOptions {
        limits: LIMITS,
        ..DecodeOptions::default()
    };
    for hint in [Some(format), None] {
        if let Ok(decoded) = decode_bytes(bytes, hint, &options, &cancel) {
            let fits = match &decoded.media {
                Media::Still(image) => within_limits(image),
                Media::Animated(animation) => animation
                    .frames
                    .iter()
                    .all(|frame| within_limits(&frame.image)),
            };
            assert!(
                fits,
                "{format:?}, {what}: the image is larger than the limits"
            );
            assert!(
                decoded.media.frame_count() > 0,
                "{format:?}, {what}: a file without frames was accepted"
            );
        }
        // The answer itself is not checked: a damaged file may still be readable.
        let _ = probe_bytes(bytes, hint);
    }
    let found = metadata::extract(bytes, format);
    if let Some(exif) = found.exif {
        read_exif(exif);
    }
}

/// Runs everything PixelIt does with an EXIF block.
fn read_exif(exif: Vec<u8>) {
    let _ = metadata::orientation(&exif);
    let mut copy = exif.clone();
    metadata::reset_orientation(&mut copy);
    metadata::strip_gps(&mut copy);
    metadata::strip_thumbnail(&mut copy);
    let _ = metadata::normalize_exif(exif);
}

#[test]
fn intact_files_are_read() {
    use_the_avif_helper();
    let cancel = CancelToken::new();
    for (format, bytes) in intact_files() {
        let decoded = decode_bytes(&bytes, None, &DecodeOptions::default(), &cancel)
            .unwrap_or_else(|error| panic!("{format:?} was not read: {error:?}"));
        assert_eq!(decoded.info.format, Some(format));
        let image = decoded.media.first().expect("a frame");
        // ICO keeps the proportions inside its own sizes; the others keep the size.
        if format != ImageFormat::Ico {
            assert_eq!(
                (image.width(), image.height()),
                (WIDTH, HEIGHT),
                "{format:?}"
            );
        }
    }
}

#[test]
fn damaged_files_get_an_answer() {
    use_the_avif_helper();
    for (number, (format, bytes)) in intact_files().into_iter().enumerate() {
        for (what, copy) in damaged_copies(&bytes, 0x9E37_79B9_7F4A_7C15 ^ number as u64) {
            read(format, &copy, &what);
        }
    }
}

#[test]
fn damaged_exif_gets_an_answer() {
    // A little-endian TIFF with one directory: orientation, GPS pointer,
    // thumbnail offset and length.
    let mut exif = b"II*\0".to_vec();
    exif.extend_from_slice(&8_u32.to_le_bytes());
    exif.extend_from_slice(&4_u16.to_le_bytes());
    for (tag, kind, value) in [
        (0x0112_u16, 3_u16, 6_u32),
        (0x8825, 4, 62),
        (0x0201, 4, 68),
        (0x0202, 4, 8),
    ] {
        exif.extend_from_slice(&tag.to_le_bytes());
        exif.extend_from_slice(&kind.to_le_bytes());
        exif.extend_from_slice(&1_u32.to_le_bytes());
        exif.extend_from_slice(&value.to_le_bytes());
    }
    exif.extend_from_slice(&0_u32.to_le_bytes());
    // An empty GPS directory and eight bytes of a thumbnail.
    exif.extend_from_slice(&0_u16.to_le_bytes());
    exif.extend_from_slice(&0_u32.to_le_bytes());
    exif.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9, 0, 0, 0, 0]);

    assert_eq!(metadata::orientation(&exif), Some(6));
    read_exif(exif.clone());
    for (_, copy) in damaged_copies(&exif, 0x2545_F491_4F6C_DD1D) {
        read_exif(copy);
    }
}
