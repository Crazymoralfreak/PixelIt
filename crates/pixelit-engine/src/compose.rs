//! Making one file out of several pictures: a collage, a PDF document or an
//! animation, and taking an animation apart.

use pixelit_codecs::{encode, EncodeOptions};
use pixelit_core::image::{Animation, Frame};
use pixelit_core::ops::draw::composite;
use pixelit_core::ops::resize::{resize, resize_to};
use pixelit_core::pipeline::{Resize, ResizeFilter, ResizeMode};
use pixelit_core::{CancelToken, Color, Depth, Error, Image, ImageFormat, Limits, Media, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CollageSpec {
    /// Pictures per row; `0` picks a near-square grid.
    pub columns: u32,
    /// Width and height of one cell in pixels.
    pub cell_width: u32,
    pub cell_height: u32,
    /// Space between cells and around the grid, in pixels.
    pub gap: u32,
    pub background: Color,
    /// Fill each cell and crop the overflow; otherwise fit inside the cell.
    pub fill: bool,
}

impl Default for CollageSpec {
    fn default() -> Self {
        Self {
            columns: 0,
            cell_width: 512,
            cell_height: 512,
            gap: 16,
            background: Color::WHITE,
            fill: true,
        }
    }
}

/// Places pictures on a grid.
pub fn collage(images: &[Image], spec: &CollageSpec, cancel: &CancelToken) -> Result<Image> {
    if images.is_empty() {
        return Err(Error::new("emptyImage"));
    }
    let count = images.len() as u32;
    let columns = if spec.columns == 0 {
        (f64::from(count).sqrt().ceil() as u32).max(1)
    } else {
        spec.columns.min(count)
    };
    let rows = count.div_ceil(columns);
    let cell_width = spec.cell_width.clamp(8, 8192);
    let cell_height = spec.cell_height.clamp(8, 8192);
    let gap = spec.gap.min(1024);
    let width =
        u64::from(columns) * u64::from(cell_width) + u64::from(columns + 1) * u64::from(gap);
    let height = u64::from(rows) * u64::from(cell_height) + u64::from(rows + 1) * u64::from(gap);
    let limits = Limits::default();
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(Error::new("tooLarge"));
    };
    limits.check_size(width, height)?;
    let mut canvas = Image::new(width, height, Depth::Eight, spec.background)?;
    for (index, image) in images.iter().enumerate() {
        cancel.check()?;
        let cell = resize(
            image,
            &Resize {
                mode: if spec.fill {
                    ResizeMode::Fill
                } else {
                    ResizeMode::Fit
                },
                width: cell_width,
                height: cell_height,
                upscale: true,
                ..Resize::default()
            },
            1.0,
        )?;
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let left =
            gap + column * (cell_width + gap) + (cell_width - cell.width().min(cell_width)) / 2;
        let top =
            gap + row * (cell_height + gap) + (cell_height - cell.height().min(cell_height)) / 2;
        composite(&mut canvas, &cell, i64::from(left), i64::from(top), 1.0);
    }
    canvas.normalize_alpha();
    Ok(canvas)
}

// ------------------------------------------------------------------- PDF

/// Number of colour components of a JPEG file, read from its frame header.
fn jpeg_components(bytes: &[u8]) -> Option<u8> {
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        let is_frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_frame {
            return bytes.get(at + 9).copied();
        }
        at += 2 + length;
    }
    None
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PdfSpec {
    /// Resolution the pictures are placed at, in dots per inch. A page is as
    /// large as its picture.
    pub dpi: f32,
    /// JPEG quality of the pages.
    pub quality: u8,
    pub title: String,
}

impl Default for PdfSpec {
    fn default() -> Self {
        Self {
            dpi: 150.0,
            quality: 88,
            title: String::new(),
        }
    }
}

struct PdfWriter {
    out: Vec<u8>,
    offsets: Vec<usize>,
}

impl PdfWriter {
    fn begin(&mut self, id: usize) {
        if self.offsets.len() <= id {
            self.offsets.resize(id + 1, 0);
        }
        self.offsets[id] = self.out.len();
        self.out
            .extend_from_slice(format!("{id} 0 obj\n").as_bytes());
    }

    fn text(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
    }

    fn end(&mut self) {
        self.out.extend_from_slice(b"\nendobj\n");
    }

    fn stream(&mut self, dictionary: &str, data: &[u8]) {
        self.text(&format!(
            "<< {dictionary} /Length {} >>\nstream\n",
            data.len()
        ));
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream");
    }
}

/// A PDF text string as UTF-16 with a byte order mark, written in hex.
fn pdf_string(text: &str) -> String {
    let mut hex = String::from("<FEFF");
    for unit in text.encode_utf16() {
        for shift in [12, 8, 4, 0] {
            hex.push(char::from(
                b"0123456789ABCDEF"[usize::from((unit >> shift) & 15)],
            ));
        }
    }
    hex.push('>');
    hex
}

/// Writes a PDF document with one picture per page. Pictures are stored as
/// JPEG; transparent areas become white.
pub fn pdf(images: &[Image], spec: &PdfSpec, cancel: &CancelToken) -> Result<Vec<u8>> {
    if images.is_empty() {
        return Err(Error::new("emptyImage"));
    }
    let dpi = f64::from(if spec.dpi.is_finite() {
        spec.dpi.clamp(36.0, 1200.0)
    } else {
        150.0
    });
    let mut options =
        EncodeOptions::new(ImageFormat::Jpeg).with_quality(spec.quality.clamp(1, 100));
    options.progressive = false;
    options.background = Color::WHITE;

    // Object ids: 1 catalog, 2 page tree, 3 info, then three per page.
    let page_id = |index: usize| 4 + index * 3;
    let mut writer = PdfWriter {
        out: Vec::new(),
        offsets: Vec::new(),
    };
    writer
        .out
        .extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    writer.begin(1);
    writer.text("<< /Type /Catalog /Pages 2 0 R >>");
    writer.end();
    let kids: Vec<String> = (0..images.len())
        .map(|index| format!("{} 0 R", page_id(index)))
        .collect();
    writer.begin(2);
    writer.text(&format!(
        "<< /Type /Pages /Count {} /Kids [{}] >>",
        images.len(),
        kids.join(" ")
    ));
    writer.end();
    writer.begin(3);
    writer.text(&format!(
        "<< /Producer {} /Title {} >>",
        pdf_string("PixelIt"),
        pdf_string(&spec.title)
    ));
    writer.end();

    for (index, image) in images.iter().enumerate() {
        cancel.check()?;
        let jpeg = encode(image, &options, cancel)?;
        let color_space = match jpeg_components(&jpeg) {
            Some(1) => "/DeviceGray",
            Some(3) => "/DeviceRGB",
            _ => return Err(Error::new("encodeFailed").detail("unexpected JPEG layout")),
        };
        let page_width = f64::from(image.width()) * 72.0 / dpi;
        let page_height = f64::from(image.height()) * 72.0 / dpi;
        let (page, picture, content) = (page_id(index), page_id(index) + 1, page_id(index) + 2);
        writer.begin(page);
        writer.text(&format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_width:.3} {page_height:.3}] \
             /Resources << /XObject << /Im0 {picture} 0 R >> >> /Contents {content} 0 R >>"
        ));
        writer.end();
        writer.begin(picture);
        writer.stream(
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {color_space} \
                 /BitsPerComponent 8 /Filter /DCTDecode",
                image.width(),
                image.height()
            ),
            &jpeg,
        );
        writer.end();
        let drawing = format!("q {page_width:.3} 0 0 {page_height:.3} 0 0 cm /Im0 Do Q");
        writer.begin(content);
        writer.stream("", drawing.as_bytes());
        writer.end();
    }

    let xref = writer.out.len();
    let count = writer.offsets.len();
    writer.text(&format!("xref\n0 {count}\n0000000000 65535 f \n"));
    for offset in writer.offsets.clone().iter().skip(1) {
        writer.text(&format!("{offset:010} 00000 n \n"));
    }
    writer.text(&format!(
        "trailer\n<< /Size {count} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    ));
    Ok(writer.out)
}

// ------------------------------------------------------------- animation

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnimationSpec {
    /// Time each frame is shown, in milliseconds.
    pub delay_ms: u32,
    /// Times the animation plays; `0` repeats forever.
    pub loop_count: u32,
    /// Size of the animation; `0` takes the size of the first frame.
    pub width: u32,
    pub height: u32,
}

impl Default for AnimationSpec {
    fn default() -> Self {
        Self {
            delay_ms: 100,
            loop_count: 0,
            width: 0,
            height: 0,
        }
    }
}

/// Builds an animation from still pictures. Every frame is brought to one
/// size, cropping the overflow around the centre.
pub fn animation(images: Vec<Image>, spec: &AnimationSpec, cancel: &CancelToken) -> Result<Media> {
    let Some(first) = images.first() else {
        return Err(Error::new("emptyImage"));
    };
    let width = if spec.width == 0 {
        first.width()
    } else {
        spec.width
    };
    let height = if spec.height == 0 {
        first.height()
    } else {
        spec.height
    };
    Limits::default().check_animation(width, height, images.len() as u64)?;
    let delay_ms = spec.delay_ms.clamp(10, 600_000);
    let mut frames = Vec::with_capacity(images.len());
    for image in images {
        cancel.check()?;
        let image = image.into_depth(Depth::Eight);
        let image = if image.width() == width && image.height() == height {
            image
        } else {
            resize(
                &image,
                &Resize {
                    mode: ResizeMode::Fill,
                    width,
                    height,
                    upscale: true,
                    ..Resize::default()
                },
                1.0,
            )?
        };
        frames.push(Frame { image, delay_ms });
    }
    Ok(Media::Animated(Animation {
        frames,
        loop_count: spec.loop_count,
    }))
}

/// The frames of an animation as still pictures.
pub fn frames(media: Media) -> Vec<Image> {
    match media {
        Media::Still(image) => vec![image],
        Media::Animated(animation) => animation
            .frames
            .into_iter()
            .map(|frame| frame.image)
            .collect(),
    }
}

/// Changes the speed of an animation: `2.0` plays twice as fast.
pub fn retime(media: &mut Media, speed: f32) {
    if let Media::Animated(animation) = media {
        let speed = if speed.is_finite() {
            speed.clamp(0.05, 20.0)
        } else {
            1.0
        };
        for frame in &mut animation.frames {
            frame.delay_ms = ((frame.delay_ms as f32 / speed).round() as u32).clamp(10, 600_000);
        }
    }
}

/// A picture reduced to at most `max_side`, used to keep collage inputs small.
pub fn reduced(image: &Image, max_side: u32) -> Result<Image> {
    let long = image.width().max(image.height());
    if long <= max_side {
        return Ok(image.clone());
    }
    let factor = f64::from(max_side) / f64::from(long);
    resize_to(
        image,
        ((f64::from(image.width()) * factor).round() as u32).max(1),
        ((f64::from(image.height()) * factor).round() as u32).max(1),
        ResizeFilter::Lanczos3,
        true,
        None,
    )
}

#[cfg(test)]
mod tests {
    use pixelit_codecs::{decode_bytes, encode_media, DecodeOptions};

    use super::*;

    fn solid(width: u32, height: u32, color: Color) -> Image {
        Image::new(width, height, Depth::Eight, color).unwrap()
    }

    #[test]
    fn collage_places_cells() {
        let images = [
            solid(100, 50, Color::rgb(255, 0, 0)),
            solid(50, 100, Color::rgb(0, 255, 0)),
            solid(80, 80, Color::rgb(0, 0, 255)),
        ];
        let spec = CollageSpec {
            cell_width: 40,
            cell_height: 40,
            gap: 10,
            ..CollageSpec::default()
        };
        let sheet = collage(&images, &spec, &CancelToken::new()).unwrap();
        // Three pictures make a 2 x 2 grid.
        assert_eq!((sheet.width(), sheet.height()), (110, 110));
        assert_eq!(sheet.pixel8(30, 30), [255, 0, 0, 255]);
        assert_eq!(sheet.pixel8(80, 30), [0, 255, 0, 255]);
        assert_eq!(sheet.pixel8(30, 80), [0, 0, 255, 255]);
        assert_eq!(sheet.pixel8(80, 80), [255, 255, 255, 255]);
        assert_eq!(sheet.pixel8(5, 5), [255, 255, 255, 255]);
        assert!(collage(&[], &spec, &CancelToken::new()).is_err());
    }

    #[test]
    fn pdf_has_a_page_per_picture() {
        let images = [
            solid(300, 150, Color::rgb(200, 30, 30)),
            solid(64, 64, Color::WHITE),
        ];
        let bytes = pdf(&images, &PdfSpec::default(), &CancelToken::new()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.7"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/MediaBox [0 0 144.000 72.000]"));
        assert_eq!(text.matches("/Filter /DCTDecode").count(), 2);
        // The cross-reference table points at the objects.
        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(bytes[xref_at..].starts_with(b"xref"));
        let table = String::from_utf8_lossy(&bytes[xref_at..]).into_owned();
        // Lines: `xref`, the object count, the free entry, then object 1.
        let first_offset: usize = table.lines().nth(3).unwrap()[..10].parse().unwrap();
        assert!(bytes[first_offset..].starts_with(b"1 0 obj"));
    }

    #[test]
    fn jpeg_components_are_read() {
        let color = encode(
            &solid(16, 16, Color::rgb(200, 30, 30)),
            &EncodeOptions::new(ImageFormat::Jpeg),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(jpeg_components(&color), Some(3));
        assert_eq!(jpeg_components(b"\xFF\xD8garbage"), None);
    }

    #[test]
    fn animation_round_trip() {
        let images = vec![
            solid(32, 32, Color::rgb(255, 0, 0)),
            solid(64, 32, Color::rgb(0, 0, 255)),
        ];
        let spec = AnimationSpec {
            delay_ms: 250,
            ..AnimationSpec::default()
        };
        let mut media = animation(images, &spec, &CancelToken::new()).unwrap();
        assert_eq!(media.frame_count(), 2);
        retime(&mut media, 2.5);
        let bytes = encode_media(
            &media,
            &EncodeOptions::new(ImageFormat::Gif),
            &CancelToken::new(),
        )
        .unwrap();
        let decoded = decode_bytes(
            &bytes,
            Some(ImageFormat::Gif),
            &DecodeOptions::default(),
            &CancelToken::new(),
        )
        .unwrap();
        let stills = frames(decoded.media);
        assert_eq!(stills.len(), 2);
        assert_eq!((stills[1].width(), stills[1].height()), (32, 32));
        assert_eq!(reduced(&stills[0], 16).unwrap().width(), 16);
    }
}
