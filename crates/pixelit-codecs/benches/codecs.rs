//! Benchmarks of encoding and decoding a 2-megapixel image.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use pixelit_codecs::{decode_bytes, encode, DecodeOptions, EncodeOptions};
use pixelit_core::pipeline::Effort;
use pixelit_core::{CancelToken, Image, ImageFormat};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

const FORMATS: [(&str, ImageFormat); 5] = [
    ("jpeg", ImageFormat::Jpeg),
    ("png", ImageFormat::Png),
    ("webp", ImageFormat::Webp),
    ("avif", ImageFormat::Avif),
    ("jxl", ImageFormat::Jxl),
];

/// Smooth gradients with a fine texture, closer to a photo than a flat fill.
fn source() -> Image {
    let mut data = Vec::with_capacity(WIDTH as usize * HEIGHT as usize * 4);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let texture = ((x * 7 + y * 13) % 17) as u8;
            data.extend_from_slice(&[
                (x * 255 / WIDTH) as u8,
                (y * 255 / HEIGHT) as u8,
                ((x + y) % 256) as u8 / 2 + texture,
                255,
            ]);
        }
    }
    let mut image = Image::from_rgba8(WIDTH, HEIGHT, data).expect("valid buffer");
    image.set_has_alpha(false);
    image
}

fn options(format: ImageFormat) -> EncodeOptions {
    let mut options = EncodeOptions::new(format);
    options.effort = Effort::Fast;
    options
}

fn encode_benches(criterion: &mut Criterion) {
    let image = source();
    let cancel = CancelToken::new();
    let mut group = criterion.benchmark_group("encode_2mp");
    group.sample_size(10);
    for (name, format) in FORMATS {
        let options = options(format);
        group.bench_function(name, |bencher| {
            bencher.iter(|| encode(black_box(&image), &options, &cancel).expect("encode"));
        });
    }
    group.finish();
}

fn decode_benches(criterion: &mut Criterion) {
    let image = source();
    let cancel = CancelToken::new();
    let settings = DecodeOptions::default();
    let mut group = criterion.benchmark_group("decode_2mp");
    group.sample_size(10);
    for (name, format) in FORMATS {
        let bytes = encode(&image, &options(format), &cancel).expect("encode");
        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                decode_bytes(black_box(&bytes), Some(format), &settings, &cancel).expect("decode")
            });
        });
    }
    group.finish();
}

criterion_group!(benches, encode_benches, decode_benches);
criterion_main!(benches);
