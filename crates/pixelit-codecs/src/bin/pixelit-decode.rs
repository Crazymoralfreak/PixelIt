//! The AVIF decoding helper as a program of its own.
//!
//! PixelIt itself is its own helper (`PixelIt.exe --decode-avif`), so this
//! program is not shipped. The tests of this crate use it to read damaged
//! AVIF files the way the application does: in a separate process.

use std::process::exit;

use pixelit_codecs::decode::avif_helper_main;

fn main() {
    exit(avif_helper_main());
}
