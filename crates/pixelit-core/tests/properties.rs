//! Properties of resizing and geometry that must hold for any input,
//! checked on generated values.

use pixelit_core::ops::{geometry, resize};
use pixelit_core::pipeline::{Anchor, Crop, CropMode, Flip, Resize, ResizeFilter, ResizeMode};
use pixelit_core::{Color, Depth, Image};
use proptest::prelude::*;

fn modes() -> impl Strategy<Value = ResizeMode> {
    prop::sample::select(vec![
        ResizeMode::Fit,
        ResizeMode::Fill,
        ResizeMode::Exact,
        ResizeMode::Percent,
        ResizeMode::LongSide,
        ResizeMode::ShortSide,
        ResizeMode::Width,
        ResizeMode::Height,
    ])
}

fn filters() -> impl Strategy<Value = ResizeFilter> {
    prop::sample::select(vec![
        ResizeFilter::Nearest,
        ResizeFilter::Box,
        ResizeFilter::Bilinear,
        ResizeFilter::Hamming,
        ResizeFilter::CatmullRom,
        ResizeFilter::Mitchell,
        ResizeFilter::Gaussian,
        ResizeFilter::Lanczos3,
    ])
}

fn anchors() -> impl Strategy<Value = Anchor> {
    prop::sample::select(vec![
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ])
}

fn depths() -> impl Strategy<Value = Depth> {
    prop::sample::select(vec![Depth::Eight, Depth::Sixteen])
}

/// Any crop step, including values far outside the `0.0..=1.0` range,
/// infinities and NaN.
fn crops() -> impl Strategy<Value = Crop> {
    (
        any::<bool>(),
        prop::array::uniform4(prop::num::f32::ANY),
        any::<u32>(),
        any::<u32>(),
        anchors(),
    )
        .prop_map(
            |(aspect, [x, y, width, height], aspect_width, aspect_height, anchor)| Crop {
                mode: if aspect {
                    CropMode::Aspect
                } else {
                    CropMode::Rect
                },
                x,
                y,
                width,
                height,
                aspect_width,
                aspect_height,
                anchor,
            },
        )
}

/// A small image with arbitrary pixels.
fn images() -> impl Strategy<Value = Image> {
    (1_u32..=12, 1_u32..=12).prop_flat_map(|(width, height)| {
        prop::collection::vec(any::<u8>(), width as usize * height as usize * 4)
            .prop_map(move |data| Image::from_rgba8(width, height, data).expect("matching buffer"))
    })
}

proptest! {
    #[test]
    fn planned_size_is_always_valid(
        src_width in 1_u32..=20_000,
        src_height in 1_u32..=20_000,
        mode in modes(),
        width in any::<u32>(),
        height in any::<u32>(),
        percent in prop::num::f32::ANY,
        upscale in any::<bool>(),
        scale in prop::num::f32::ANY,
    ) {
        let spec = Resize { mode, width, height, percent, upscale, ..Resize::default() };
        let planned = resize::plan(src_width, src_height, &spec, scale);
        prop_assert!((1..=resize::MAX_SIDE).contains(&planned.width));
        prop_assert!((1..=resize::MAX_SIDE).contains(&planned.height));
        if let Some([left, top, crop_width, crop_height]) = planned.crop {
            prop_assert!(left >= 0.0 && top >= 0.0, "{planned:?}");
            prop_assert!(crop_width > 0.0 && crop_height > 0.0, "{planned:?}");
            prop_assert!(left + crop_width <= f64::from(src_width) + 1e-6, "{planned:?}");
            prop_assert!(top + crop_height <= f64::from(src_height) + 1e-6, "{planned:?}");
        }
    }

    #[test]
    fn nothing_grows_while_enlarging_is_off(
        src_width in 1_u32..=20_000,
        src_height in 1_u32..=20_000,
        mode in modes(),
        width in 1_u32..=40_000,
        height in 1_u32..=40_000,
        percent in 0.0_f32..=1000.0,
    ) {
        prop_assume!(mode != ResizeMode::Exact);
        let spec = Resize { mode, width, height, percent, upscale: false, ..Resize::default() };
        let planned = resize::plan(src_width, src_height, &spec, 1.0);
        prop_assert!(planned.width <= src_width, "{planned:?}");
        prop_assert!(planned.height <= src_height, "{planned:?}");
    }

    #[test]
    fn fit_keeps_the_aspect_ratio(
        src_width in 16_u32..=2000,
        src_height in 16_u32..=2000,
        width in 64_u32..=8000,
        height in 64_u32..=8000,
    ) {
        let spec = Resize { mode: ResizeMode::Fit, width, height, upscale: true, ..Resize::default() };
        let planned = resize::plan(src_width, src_height, &spec, 1.0);
        prop_assert!(planned.width <= width && planned.height <= height, "{planned:?}");
        // One of the sides touches the box; both differ from the exact
        // proportion by rounding only.
        prop_assert!(planned.width == width || planned.height == height, "{planned:?}");
        let factor = (f64::from(width) / f64::from(src_width))
            .min(f64::from(height) / f64::from(src_height));
        let exact_width = f64::from(src_width) * factor;
        let exact_height = f64::from(src_height) * factor;
        prop_assert!((exact_width - f64::from(planned.width)).abs() <= 0.5 + 1e-6, "{planned:?}");
        prop_assert!((exact_height - f64::from(planned.height)).abs() <= 0.5 + 1e-6, "{planned:?}");
    }

    #[test]
    fn crop_box_stays_inside_the_image(
        width in 1_u32..=20_000,
        height in 1_u32..=20_000,
        spec in crops(),
    ) {
        let rect = geometry::crop_rect(width, height, &spec);
        prop_assert!(rect.width >= 1 && rect.height >= 1, "{rect:?}");
        prop_assert!(u64::from(rect.x) + u64::from(rect.width) <= u64::from(width), "{rect:?}");
        prop_assert!(u64::from(rect.y) + u64::from(rect.height) <= u64::from(height), "{rect:?}");
    }

    #[test]
    fn crop_returns_the_planned_box(image in images(), spec in crops()) {
        let rect = geometry::crop_rect(image.width(), image.height(), &spec);
        let cropped = geometry::crop(&image, &spec).expect("a box inside the image");
        prop_assert_eq!((cropped.width(), cropped.height()), (rect.width, rect.height));
        prop_assert_eq!(cropped.pixel8(0, 0), image.pixel8(rect.x, rect.y));
        prop_assert_eq!(
            cropped.pixel8(rect.width - 1, rect.height - 1),
            image.pixel8(rect.x + rect.width - 1, rect.y + rect.height - 1)
        );
    }

    #[test]
    fn mirroring_twice_changes_nothing(
        image in images(),
        horizontal in any::<bool>(),
        vertical in any::<bool>(),
    ) {
        let spec = Flip { horizontal, vertical };
        let once = geometry::flip(image.clone(), &spec);
        prop_assert_eq!((once.width(), once.height()), (image.width(), image.height()));
        if horizontal && !vertical {
            prop_assert_eq!(once.pixel8(0, 0), image.pixel8(image.width() - 1, 0));
        }
        if vertical && !horizontal {
            prop_assert_eq!(once.pixel8(0, 0), image.pixel8(0, image.height() - 1));
        }
        prop_assert_eq!(geometry::flip(once, &spec), image);
    }

    #[test]
    fn quarter_turns_add_up_to_a_full_turn(image in images(), quarters in 0_u8..=7) {
        let turned = geometry::rotate_quarters(image.clone(), quarters);
        if quarters % 2 == 1 {
            prop_assert_eq!((turned.width(), turned.height()), (image.height(), image.width()));
        } else {
            prop_assert_eq!((turned.width(), turned.height()), (image.width(), image.height()));
        }
        if quarters % 4 == 1 {
            // A clockwise quarter turn moves the bottom-left corner to the top-left.
            prop_assert_eq!(turned.pixel8(0, 0), image.pixel8(0, image.height() - 1));
        }
        let back = geometry::rotate_quarters(turned, (8 - quarters) % 4);
        prop_assert_eq!(back, image);
    }
}

proptest! {
    // Every case resamples an image, so there are fewer of them.
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn resizing_keeps_a_flat_colour(
        src_width in 1_u32..=48,
        src_height in 1_u32..=48,
        width in 1_u32..=64,
        height in 1_u32..=64,
        filter in filters(),
        gamma_correct in any::<bool>(),
        depth in depths(),
        colour in prop::array::uniform3(any::<u8>()),
    ) {
        let [red, green, blue] = colour;
        let image = Image::new(src_width, src_height, depth, Color::rgb(red, green, blue))
            .expect("a small image");
        let resized = resize::resize_to(&image, width, height, filter, gamma_correct, None)
            .expect("a resize within the limits");
        prop_assert_eq!((resized.width(), resized.height()), (width, height));
        prop_assert_eq!(resized.depth(), depth);
        for (x, y) in [(0, 0), (width - 1, 0), (0, height - 1), (width / 2, height / 2)] {
            let [r, g, b, a] = resized.pixel8(x, y);
            prop_assert!(r.abs_diff(red) <= 2, "red {r} instead of {red}");
            prop_assert!(g.abs_diff(green) <= 2, "green {g} instead of {green}");
            prop_assert!(b.abs_diff(blue) <= 2, "blue {b} instead of {blue}");
            prop_assert_eq!(a, 255);
        }
    }

    #[test]
    fn resize_step_never_fails_on_small_images(
        image in images(),
        mode in modes(),
        width in 0_u32..=64,
        height in 0_u32..=64,
        percent in prop::num::f32::ANY,
        filter in filters(),
        gamma_correct in any::<bool>(),
        upscale in any::<bool>(),
    ) {
        // An unbounded percentage would ask for a 65535-pixel side.
        let percent = if percent.is_finite() { percent.clamp(-10.0, 600.0) } else { percent };
        let spec = Resize { mode, width, height, percent, filter, gamma_correct, upscale };
        let planned = resize::plan(image.width(), image.height(), &spec, 1.0);
        prop_assume!(planned.width <= 512 && planned.height <= 512);
        let resized = resize::resize(&image, &spec, 1.0).expect("a planned resize");
        prop_assert_eq!((resized.width(), resized.height()), (planned.width, planned.height));
    }
}
