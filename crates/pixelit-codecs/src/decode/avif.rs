//! AVIF decoding with avif-decode (rav1d).
//!
//! rav1d keeps the C interface of dav1d, and a panic inside it cannot cross
//! that boundary: on some damaged bitstreams the decoder ends the whole
//! process, which no `catch_unwind` prevents. A program that names a helper
//! with [`set_helper`] therefore decodes AVIF pixels in a short-lived child
//! process, and a crash there is an ordinary "bad file" error here. Without a
//! helper the file is decoded in this process.

use std::env;
use std::io::{self, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::panic::catch_unwind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use avif_decode::{Decoder, Image as AvifImage};
use pixelit_core::image::Pixels;
use pixelit_core::{Error, Image, Limits, Result};

use super::{bad, bmff_size, Loaded};
use crate::info::DecodeOptions;

/// Bits reported for high-depth AVIF files; the decoder scales samples to 16 bits.
const DEEP_BITS: u8 = 10;

/// First argument of a program started as the decoding helper. It is followed
/// by the largest side and the largest pixel count the caller accepts; the
/// file comes on the standard input and the pixels leave on the standard output.
pub const HELPER_ARGUMENT: &str = "--decode-avif";

/// Start of the helper's answer. After it: width and height (`u32`), bits of
/// the source, bytes per sample, whether alpha is used, then RGBA samples.
const MAGIC: &[u8; 4] = b"PXIA";
const HEADER: usize = MAGIC.len() + 4 + 4 + 3;

const EXIT_DONE: i32 = 0;
const EXIT_BAD_FILE: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_TOO_LARGE: i32 = 3;

/// `CREATE_NO_WINDOW`: the helper never opens a console of its own.
#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

static HELPER: OnceLock<PathBuf> = OnceLock::new();

/// Names the program that decodes AVIF files in its own process. The program
/// has to call [`helper_main`] when its first argument is [`HELPER_ARGUMENT`].
/// The first call wins.
pub fn set_helper(program: PathBuf) {
    let _ = HELPER.set(program);
}

fn size(width: usize, height: usize) -> Result<(u32, u32)> {
    let width = u32::try_from(width).map_err(|_| Error::new("tooLarge"))?;
    let height = u32::try_from(height).map_err(|_| Error::new("tooLarge"))?;
    Ok((width, height))
}

/// Decodes in this process. Returns the image and the bits of the source.
fn decode_here(bytes: &[u8]) -> Result<(Image, u8)> {
    let decoded = Decoder::from_avif(bytes)
        .map_err(bad)?
        .to_image()
        .map_err(bad)?;
    Ok(match decoded {
        AvifImage::Rgb8(pixels) => {
            let (data, width, height) = pixels.into_contiguous_buf();
            let (width, height) = size(width, height)?;
            (
                Image::from_channels8(width, height, 3, bytemuck::cast_slice(&data))?,
                8,
            )
        }
        AvifImage::Rgba8(pixels) => {
            let (data, width, height) = pixels.into_contiguous_buf();
            let (width, height) = size(width, height)?;
            (
                Image::from_channels8(width, height, 4, bytemuck::cast_slice(&data))?,
                8,
            )
        }
        AvifImage::Gray8(pixels) => {
            let (data, width, height) = pixels.into_contiguous_buf();
            let (width, height) = size(width, height)?;
            (
                Image::from_channels8(width, height, 1, bytemuck::cast_slice(&data))?,
                8,
            )
        }
        AvifImage::Rgb16(pixels) => {
            let (data, width, height) = pixels.into_contiguous_buf();
            let (width, height) = size(width, height)?;
            (
                Image::from_channels16(width, height, 3, bytemuck::cast_slice(&data))?,
                DEEP_BITS,
            )
        }
        AvifImage::Rgba16(pixels) => {
            let (data, width, height) = pixels.into_contiguous_buf();
            let (width, height) = size(width, height)?;
            (
                Image::from_channels16(width, height, 4, bytemuck::cast_slice(&data))?,
                DEEP_BITS,
            )
        }
        AvifImage::Gray16(pixels) => {
            let (data, width, height) = pixels.into_contiguous_buf();
            let (width, height) = size(width, height)?;
            (
                Image::from_channels16(width, height, 1, bytemuck::cast_slice(&data))?,
                DEEP_BITS,
            )
        }
    })
}

/// The answer of the helper for a decoded image.
fn pack(image: &Image, bits: u8) -> Vec<u8> {
    let (sample_bytes, samples): (u8, &[u8]) = match image.pixels() {
        Pixels::U8(data) => (1, data),
        Pixels::U16(data) => (2, bytemuck::cast_slice(data)),
    };
    let mut answer = Vec::with_capacity(HEADER + samples.len());
    answer.extend_from_slice(MAGIC);
    answer.extend_from_slice(&image.width().to_le_bytes());
    answer.extend_from_slice(&image.height().to_le_bytes());
    answer.extend_from_slice(&[bits, sample_bytes, u8::from(image.has_alpha())]);
    answer.extend_from_slice(samples);
    answer
}

/// Reads the answer of the helper back into an image.
fn unpack(answer: &[u8], limits: &Limits) -> Result<(Image, u8)> {
    let broken = || bad("the AVIF decoder gave an incomplete answer");
    if answer.len() < HEADER || !answer.starts_with(MAGIC) {
        return Err(broken());
    }
    let field = |at: usize| -> Result<u32> {
        let raw: [u8; 4] = answer[at..at + 4].try_into().map_err(|_| broken())?;
        Ok(u32::from_le_bytes(raw))
    };
    let (width, height) = (field(4)?, field(8)?);
    let [bits, sample_bytes, alpha] = [answer[12], answer[13], answer[14]];
    limits.check_size(width, height)?;
    let samples = &answer[HEADER..];
    let mut image = match sample_bytes {
        1 => Image::from_rgba8(width, height, samples.to_vec())?,
        2 if samples.len().is_multiple_of(2) => {
            Image::from_rgba16(width, height, bytemuck::pod_collect_to_vec(samples))?
        }
        _ => return Err(broken()),
    };
    image.set_has_alpha(alpha != 0);
    Ok((image, bits))
}

/// Decodes in a child process, so a decoder that stops the process stops
/// only the helper.
fn decode_in_helper(helper: &Path, bytes: &[u8], limits: &Limits) -> Result<(Image, u8)> {
    let mut command = Command::new(helper);
    command
        .arg(HELPER_ARGUMENT)
        .arg(limits.max_side.to_string())
        .arg(limits.max_pixels.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(NO_WINDOW);
    let mut child = command.spawn()?;
    // The helper reads the whole file before it answers, so writing first
    // cannot wait for its output. A helper that stopped early closes the
    // pipe; its exit code tells the rest.
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(bytes);
    }
    let most = (HEADER as u64).saturating_add(limits.max_pixels.saturating_mul(8));
    let mut answer = Vec::new();
    let received = match child.stdout.take() {
        Some(output) => output.take(most).read_to_end(&mut answer).is_ok(),
        None => false,
    };
    let status = child.wait()?;
    match status.code() {
        Some(EXIT_DONE) if received => unpack(&answer, limits),
        Some(EXIT_TOO_LARGE) => Err(Error::new("tooLarge")),
        _ => Err(bad("the AVIF decoder stopped on this file")),
    }
}

/// The body of the helper process: reads an AVIF file from the standard
/// input, writes its pixels to the standard output and returns the exit code.
pub fn helper_main() -> i32 {
    let mut arguments = env::args().skip(2);
    let (Some(max_side), Some(max_pixels)) = (
        arguments.next().and_then(|text| text.parse::<u32>().ok()),
        arguments.next().and_then(|text| text.parse::<u64>().ok()),
    ) else {
        return EXIT_USAGE;
    };
    let limits = Limits {
        max_side,
        max_pixels,
        ..Limits::default()
    };
    let mut bytes = Vec::new();
    if io::stdin().lock().read_to_end(&mut bytes).is_err() {
        return EXIT_USAGE;
    }
    let Ok(decoded) = catch_unwind(|| decode_here(&bytes)) else {
        return EXIT_BAD_FILE;
    };
    let Ok((image, bits)) = decoded else {
        return EXIT_BAD_FILE;
    };
    if limits.check_size(image.width(), image.height()).is_err() {
        return EXIT_TOO_LARGE;
    }
    let mut output = io::stdout().lock();
    if output.write_all(&pack(&image, bits)).is_err() || output.flush().is_err() {
        return EXIT_BAD_FILE;
    }
    EXIT_DONE
}

pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    // The size is checked before the decoder allocates its planes.
    if let Some((width, height)) = bmff_size(bytes) {
        options.limits.check_size(width, height)?;
    }
    let (image, bits) = match HELPER.get() {
        Some(helper) => decode_in_helper(helper, bytes, &options.limits)?,
        None => decode_here(bytes)?,
    };
    options.limits.check_size(image.width(), image.height())?;
    Ok(Loaded::still(image, bits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_of_the_helper_is_read_back() {
        let limits = Limits::default();
        let mut deep =
            Image::from_rgba16(2, 1, vec![1, 2, 3, 65535, 400, 500, 600, 65535]).unwrap();
        deep.set_has_alpha(false);
        let (image, bits) = unpack(&pack(&deep, 10), &limits).unwrap();
        assert_eq!((image.width(), image.height(), bits), (2, 1, 10));
        assert!(!image.has_alpha());
        assert_eq!(image.to_rgba16(), deep.to_rgba16());

        let plain = Image::from_rgba8(1, 2, vec![9, 8, 7, 6, 5, 4, 3, 2]).unwrap();
        let (image, bits) = unpack(&pack(&plain, 8), &limits).unwrap();
        assert_eq!(bits, 8);
        assert!(image.has_alpha());
        assert_eq!(image.to_rgba8(), plain.to_rgba8());
    }

    #[test]
    fn a_broken_answer_is_an_error() {
        let limits = Limits::default();
        let image = Image::from_rgba8(2, 2, vec![0; 16]).unwrap();
        let answer = pack(&image, 8);
        assert!(unpack(&answer[..answer.len() - 1], &limits).is_err());
        assert!(unpack(&answer[..HEADER - 1], &limits).is_err());
        assert!(unpack(b"", &limits).is_err());
        let mut wrong = answer.clone();
        wrong[0] = b'?';
        assert!(unpack(&wrong, &limits).is_err());
        let small = Limits {
            max_side: 1,
            ..Limits::default()
        };
        assert_eq!(unpack(&answer, &small).unwrap_err().code, "tooLarge");
    }
}
