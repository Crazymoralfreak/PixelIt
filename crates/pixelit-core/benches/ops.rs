//! Benchmarks of the core operations on a 12-megapixel image.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use pixelit_core::ops::{adjust, filter, geometry, resize};
use pixelit_core::pipeline::{Adjust, Filter, FilterKind, ResizeFilter};
use pixelit_core::{Color, Depth, Image};

const WIDTH: u32 = 4000;
const HEIGHT: u32 = 3000;

fn source(depth: Depth) -> Image {
    let mut data = Vec::with_capacity(WIDTH as usize * HEIGHT as usize * 4);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            data.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]);
        }
    }
    Image::from_rgba8(WIDTH, HEIGHT, data)
        .expect("valid buffer")
        .into_depth(depth)
}

fn resize_benches(criterion: &mut Criterion) {
    let image = source(Depth::Eight);
    let mut group = criterion.benchmark_group("resize_12mp_to_1920");
    group.sample_size(10);
    for (name, gamma) in [("lanczos3", false), ("lanczos3_linear", true)] {
        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                resize::resize_to(
                    black_box(&image),
                    1920,
                    1440,
                    ResizeFilter::Lanczos3,
                    gamma,
                    None,
                )
                .expect("resize")
            });
        });
    }
    group.finish();
}

fn pixel_benches(criterion: &mut Criterion) {
    let image = source(Depth::Eight);
    let mut group = criterion.benchmark_group("ops_12mp");
    group.sample_size(10);

    let tone = Adjust {
        exposure: 0.3,
        contrast: 20.0,
        saturation: 15.0,
        ..Adjust::default()
    };
    group.bench_function("adjust", |bencher| {
        bencher.iter(|| {
            let mut copy = image.clone();
            adjust::adjust(&mut copy, black_box(&tone));
            copy
        });
    });

    let sharpen = Filter {
        kind: FilterKind::Sharpen,
        amount: 60.0,
        radius: 1.2,
    };
    group.bench_function("sharpen", |bencher| {
        bencher.iter(|| {
            let mut copy = image.clone();
            filter::filter(&mut copy, black_box(&sharpen), 1.0);
            copy
        });
    });

    group.bench_function("blur_sigma_20", |bencher| {
        bencher.iter(|| {
            let mut copy = image.clone();
            filter::gaussian_blur(&mut copy, 20.0);
            copy
        });
    });

    group.bench_function("rotate_90", |bencher| {
        bencher.iter(|| geometry::rotate_quarters(image.clone(), 1));
    });

    group.bench_function("rotate_7_degrees", |bencher| {
        bencher.iter(|| {
            geometry::rotate_by(image.clone(), 7.0, true, Color::TRANSPARENT).expect("rotate")
        });
    });
    group.finish();
}

criterion_group!(benches, resize_benches, pixel_benches);
criterion_main!(benches);
