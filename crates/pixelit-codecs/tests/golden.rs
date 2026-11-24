//! Golden samples: the result of every operation is compared with a stored
//! picture in `tests/golden`. A difference within the tolerance (another
//! processor, another version of a library) passes; a visible change fails.
//!
//! Samples are written by running the test with `PIXELIT_BLESS=1`, which is
//! what the "Golden samples" workflow does. After an intended change of an
//! operation the samples are written again and reviewed as pictures.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use pixelit_core::pipeline::{
    Adjust, Anchor, Annotate, ApplyContext, Crop, CropMode, Filter, FilterKind, Flip, Op, Resize,
    ResizeFilter, ResizeMode, Resources, Rotate, Shape, ShapeKind, Watermark, WatermarkKind,
};
use pixelit_core::text::Font;
use pixelit_core::{CancelToken, Color, Depth, Error, Image, Result};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;

/// Largest accepted mean squared difference of 8-bit samples (about 45 dB).
const TOLERANCE: f64 = 2.0;

/// The picture every operation starts from: gradients, a checkerboard, sharp
/// colour blocks and a transparent corner. Whole numbers only, so it is the
/// same on every machine.
fn source() -> Image {
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut pixel = [
                (x * 255 / (WIDTH - 1)) as u8,
                (y * 255 / (HEIGHT - 1)) as u8,
                ((x + y) * 255 / (WIDTH + HEIGHT - 2)) as u8,
                255,
            ];
            if x >= 100 && y < 40 {
                let light = (x / 4 + y / 4) % 2 == 0;
                pixel = if light {
                    [235, 235, 235, 255]
                } else {
                    [20, 20, 20, 255]
                };
            }
            if (20..60).contains(&x) && (70..100).contains(&y) {
                pixel = [220, 40, 40, 255];
            }
            if (70..110).contains(&x) && (80..110).contains(&y) {
                pixel = [40, 160, 220, 255];
            }
            let (dx, dy) = (i64::from(x) - 130, i64::from(y) - 85);
            if dx * dx + dy * dy <= 20 * 20 {
                pixel = [250, 210, 40, 255];
            }
            if x < 24 && y < 18 {
                pixel[3] = ((x + y) * 6) as u8;
            }
            data.extend_from_slice(&pixel);
        }
    }
    Image::from_rgba8(WIDTH, HEIGHT, data).expect("matching buffer")
}

/// The picture drawn as an image watermark.
fn mark() -> Image {
    let (width, height) = (32_u32, 16_u32);
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let border = x < 2 || y < 2 || x >= width - 2 || y >= height - 2;
            data.extend_from_slice(if border {
                &[255, 255, 255, 255]
            } else {
                &[30, 30, 30, 160]
            });
        }
    }
    Image::from_rgba8(width, height, data).expect("matching buffer")
}

/// Resources of the samples: one picture and no fonts, because the fonts of
/// a machine are not part of the repository.
struct Fixtures;

impl Resources for Fixtures {
    fn font(&self, name: &str) -> Result<Font> {
        Err(Error::new("fontNotFound").with("name", name))
    }

    fn image(&self, path: &str) -> Result<Image> {
        if path == "mark" {
            Ok(mark())
        } else {
            Err(Error::new("notFound").with("path", path))
        }
    }
}

fn resize(mode: ResizeMode, width: u32, height: u32, filter: ResizeFilter, linear: bool) -> Op {
    Op::Resize(Resize {
        mode,
        width,
        height,
        filter,
        gamma_correct: linear,
        upscale: true,
        ..Resize::default()
    })
}

fn filter(kind: FilterKind, amount: f32, radius: f32) -> Op {
    Op::Filter(Filter {
        kind,
        amount,
        radius,
    })
}

fn shape(kind: ShapeKind, corners: [f32; 4], color: Color) -> Shape {
    let [x1, y1, x2, y2] = corners;
    Shape {
        kind,
        x1,
        y1,
        x2,
        y2,
        color,
        width: 2.5,
        ..Shape::default()
    }
}

/// Every sample: its file name and the steps that produce it.
fn cases() -> Vec<(&'static str, Vec<Op>)> {
    let red = Color::rgb(229, 57, 53);
    let green = Color::rgb(67, 160, 71);
    let yellow = Color::rgb(253, 216, 53);
    vec![
        (
            "resize_fit_lanczos",
            vec![resize(
                ResizeMode::Fit,
                64,
                64,
                ResizeFilter::Lanczos3,
                false,
            )],
        ),
        (
            "resize_fit_lanczos_linear",
            vec![resize(
                ResizeMode::Fit,
                64,
                64,
                ResizeFilter::Lanczos3,
                true,
            )],
        ),
        (
            "resize_fill_bilinear",
            vec![resize(
                ResizeMode::Fill,
                72,
                72,
                ResizeFilter::Bilinear,
                false,
            )],
        ),
        (
            "resize_exact_nearest",
            vec![resize(
                ResizeMode::Exact,
                240,
                90,
                ResizeFilter::Nearest,
                false,
            )],
        ),
        (
            "resize_enlarge_mitchell",
            vec![resize(
                ResizeMode::Width,
                240,
                0,
                ResizeFilter::Mitchell,
                true,
            )],
        ),
        (
            "crop_rect",
            vec![Op::Crop(Crop {
                mode: CropMode::Rect,
                x: 0.25,
                y: 0.2,
                width: 0.5,
                height: 0.6,
                ..Crop::default()
            })],
        ),
        (
            "crop_aspect_right",
            vec![Op::Crop(Crop {
                mode: CropMode::Aspect,
                aspect_width: 1,
                aspect_height: 1,
                anchor: Anchor::Right,
                ..Crop::default()
            })],
        ),
        (
            "rotate_quarter",
            vec![Op::Rotate(Rotate {
                angle: 90.0,
                expand: true,
                background: Color::TRANSPARENT,
            })],
        ),
        (
            "rotate_seven_degrees",
            vec![Op::Rotate(Rotate {
                angle: 7.0,
                expand: true,
                background: Color::TRANSPARENT,
            })],
        ),
        (
            "rotate_back_twenty_degrees",
            vec![Op::Rotate(Rotate {
                angle: -20.0,
                expand: false,
                background: Color::rgb(40, 44, 52),
            })],
        ),
        (
            "flip_both",
            vec![Op::Flip(Flip {
                horizontal: true,
                vertical: true,
            })],
        ),
        (
            "adjust_tone",
            vec![Op::Adjust(Adjust {
                exposure: 0.5,
                brightness: 10.0,
                contrast: 25.0,
                highlights: -30.0,
                shadows: 30.0,
                ..Adjust::default()
            })],
        ),
        (
            "adjust_colour",
            vec![Op::Adjust(Adjust {
                saturation: -20.0,
                vibrance: 40.0,
                temperature: 30.0,
                tint: -15.0,
                ..Adjust::default()
            })],
        ),
        (
            "adjust_curve",
            vec![Op::Adjust(Adjust {
                curve: vec![[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]],
                ..Adjust::default()
            })],
        ),
        (
            "filter_sharpen",
            vec![filter(FilterKind::Sharpen, 70.0, 1.5)],
        ),
        ("filter_blur", vec![filter(FilterKind::Blur, 50.0, 3.0)]),
        (
            "filter_denoise",
            vec![filter(FilterKind::Denoise, 60.0, 1.5)],
        ),
        ("filter_grain", vec![filter(FilterKind::Grain, 40.0, 1.0)]),
        (
            "filter_vignette",
            vec![filter(FilterKind::Vignette, 60.0, 1.0)],
        ),
        (
            "filter_grayscale",
            vec![filter(FilterKind::Grayscale, 100.0, 1.0)],
        ),
        ("filter_sepia", vec![filter(FilterKind::Sepia, 80.0, 1.0)]),
        (
            "filter_invert",
            vec![filter(FilterKind::Invert, 100.0, 1.0)],
        ),
        (
            "watermark_picture",
            vec![Op::Watermark(Watermark {
                kind: WatermarkKind::Image,
                image: String::from("mark"),
                scale: 25.0,
                anchor: Anchor::BottomRight,
                margin: 4.0,
                opacity: 80.0,
                ..Watermark::default()
            })],
        ),
        (
            "watermark_tiled",
            vec![Op::Watermark(Watermark {
                kind: WatermarkKind::Image,
                image: String::from("mark"),
                scale: 15.0,
                opacity: 50.0,
                rotation: -30.0,
                tile: true,
                tile_gap: 80.0,
                ..Watermark::default()
            })],
        ),
        (
            "annotate_shapes",
            vec![Op::Annotate(Annotate {
                shapes: vec![
                    shape(ShapeKind::Arrow, [0.1, 0.85, 0.45, 0.4], red),
                    shape(ShapeKind::Line, [0.05, 0.1, 0.5, 0.2], green),
                    shape(ShapeKind::Rect, [0.55, 0.1, 0.9, 0.4], red),
                    shape(ShapeKind::Ellipse, [0.6, 0.5, 0.95, 0.9], green),
                    shape(ShapeKind::Highlight, [0.1, 0.45, 0.5, 0.6], yellow),
                    Shape {
                        fill: true,
                        ..shape(ShapeKind::Rect, [0.02, 0.02, 0.12, 0.1], green)
                    },
                    Shape {
                        points: vec![
                            [0.2, 0.7],
                            [0.3, 0.6],
                            [0.4, 0.75],
                            [0.5, 0.65],
                            [0.55, 0.9],
                        ],
                        ..shape(ShapeKind::Pen, [0.0, 0.0, 0.0, 0.0], red)
                    },
                ],
            })],
        ),
        (
            "annotate_hidden_areas",
            vec![Op::Annotate(Annotate {
                shapes: vec![
                    shape(ShapeKind::Blur, [0.6, 0.0, 1.0, 0.35], red),
                    shape(ShapeKind::Pixelate, [0.1, 0.55, 0.7, 0.95], red),
                ],
            })],
        ),
        (
            "chain_resize_adjust_sharpen",
            vec![
                resize(ResizeMode::Fit, 96, 96, ResizeFilter::Lanczos3, true),
                Op::Adjust(Adjust {
                    contrast: 15.0,
                    saturation: 10.0,
                    ..Adjust::default()
                }),
                filter(FilterKind::Sharpen, 40.0, 1.0),
            ],
        ),
    ]
}

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn run(input: &Image, ops: &[Op]) -> Result<Image> {
    let cancel = CancelToken::new();
    let context = ApplyContext::new(&Fixtures, &cancel);
    ops.iter().try_fold(input.clone(), |image, op| {
        pixelit_core::pipeline::apply_op(image, op, &context)
    })
}

/// Compares a result with its stored sample; `Err` says what is wrong.
fn compare(path: &Path, result: &Image) -> std::result::Result<(), String> {
    let stored = image::open(path)
        .map_err(|error| format!("the sample cannot be read: {error}"))?
        .into_rgba8();
    let size = (result.width(), result.height());
    if stored.dimensions() != size {
        return Err(format!(
            "size {size:?}, the sample is {:?}",
            stored.dimensions()
        ));
    }
    let actual = result.to_rgba8();
    let expected = stored.into_raw();
    let total: u64 = actual
        .iter()
        .zip(&expected)
        .map(|(a, b)| u64::from(a.abs_diff(*b)).pow(2))
        .sum();
    let error = total as f64 / actual.len() as f64;
    if error > TOLERANCE {
        return Err(format!(
            "mean squared difference {error:.2}, allowed {TOLERANCE}"
        ));
    }
    Ok(())
}

fn check(name: &str, result: &Image, bless: bool) -> std::result::Result<(), String> {
    let path = folder().join(format!("{name}.png"));
    if bless {
        fs::create_dir_all(folder()).map_err(|error| error.to_string())?;
        return image::save_buffer(
            &path,
            &result.to_rgba8(),
            result.width(),
            result.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| format!("the sample cannot be written: {error}"));
    }
    if !path.is_file() {
        return Err(String::from(
            "there is no sample; run the \"Golden samples\" workflow or the test with PIXELIT_BLESS=1",
        ));
    }
    compare(&path, result)
}

#[test]
fn golden_samples_match() {
    let bless = env::var_os("PIXELIT_BLESS").is_some_and(|value| !value.is_empty());
    let mut problems = Vec::new();
    for depth in [Depth::Eight, Depth::Sixteen] {
        let input = source().into_depth(depth);
        for (name, ops) in cases() {
            let outcome = run(&input, &ops)
                .map_err(|error| format!("the steps failed: {error:?}"))
                .and_then(|result| {
                    // Both depths must give the same picture, so they share the
                    // sample; it is written from the 8-bit run.
                    check(name, &result, bless && depth == Depth::Eight)
                });
            if let Err(problem) = outcome {
                problems.push(format!("{name} ({} bit): {problem}", depth.bits()));
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn every_sample_belongs_to_a_case() {
    let Ok(entries) = fs::read_dir(folder()) else {
        // No samples yet: `golden_samples_match` reports that.
        return;
    };
    let names: Vec<&str> = cases().iter().map(|(name, _)| *name).collect();
    for entry in entries.flatten() {
        let path = entry.path();
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("");
        assert!(names.contains(&stem), "{} has no case", path.display());
    }
}
