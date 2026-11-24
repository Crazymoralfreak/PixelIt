//! Face geometry: reading the detector's answer, aligning a face to the
//! template the restoration model expects and blending the result back.

use serde::Serialize;

use pixelit_core::{Error, Result};

use crate::worker::Tensor;

/// Detections below this confidence are dropped.
pub const SCORE_THRESHOLD: f32 = 0.7;

/// Boxes overlapping a better one by more than this are dropped.
const OVERLAP_THRESHOLD: f32 = 0.3;

const STRIDES: [usize; 3] = [8, 16, 32];

/// Positions of the eyes, the nose and the mouth corners in a 512-pixel
/// aligned face, as the FFHQ data set defines them.
const TEMPLATE: [[f32; 2]; 5] = [
    [192.98138, 239.94708],
    [318.90277, 240.1936],
    [256.63416, 314.01935],
    [201.26117, 371.41043],
    [313.08905, 371.15118],
];

/// Share of the aligned face over which the restored picture fades in.
const FEATHER: f32 = 0.1;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub score: f32,
    /// Eyes, nose tip and mouth corners, left to right as seen in the picture.
    pub points: [[f32; 2]; 5],
}

impl Face {
    /// Multiplies every coordinate, to move between picture sizes.
    pub fn rescale(&mut self, factor: f32) {
        self.x *= factor;
        self.y *= factor;
        self.width *= factor;
        self.height *= factor;
        for point in &mut self.points {
            point[0] *= factor;
            point[1] *= factor;
        }
    }

    fn overlap(&self, other: &Self) -> f32 {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        let shared = (right - left).max(0.0) * (bottom - top).max(0.0);
        let total = self.width * self.height + other.width * other.height - shared;
        if total > 0.0 {
            shared / total
        } else {
            0.0
        }
    }
}

fn output<'a>(outputs: &'a [Tensor], name: &str, fallback: usize) -> Result<&'a Tensor> {
    outputs
        .iter()
        .find(|tensor| tensor.name == name)
        .or_else(|| outputs.get(fallback))
        .ok_or_else(|| Error::new("aiModelFailed").detail("the detector returned too little"))
}

/// Reads YuNet's answer for a square input of `size`: faces best first,
/// overlapping duplicates removed.
pub fn decode(outputs: &[Tensor], size: usize, threshold: f32) -> Result<Vec<Face>> {
    let mut faces = Vec::new();
    for (level, stride) in STRIDES.into_iter().enumerate() {
        let columns = size / stride;
        let rows = size / stride;
        let count = rows * columns;
        let class = output(outputs, &format!("cls_{stride}"), level)?;
        let object = output(outputs, &format!("obj_{stride}"), 3 + level)?;
        let boxes = output(outputs, &format!("bbox_{stride}"), 6 + level)?;
        let points = output(outputs, &format!("kps_{stride}"), 9 + level)?;
        if class.data.len() < count
            || object.data.len() < count
            || boxes.data.len() < count * 4
            || points.data.len() < count * 10
        {
            return Err(Error::new("aiModelFailed").detail("the detector returned too little"));
        }
        let step = stride as f32;
        for index in 0..count {
            let score =
                (class.data[index].clamp(0.0, 1.0) * object.data[index].clamp(0.0, 1.0)).sqrt();
            if score < threshold {
                continue;
            }
            let column = (index % columns) as f32;
            let row = (index / columns) as f32;
            let centre_x = (column + boxes.data[index * 4]) * step;
            let centre_y = (row + boxes.data[index * 4 + 1]) * step;
            let width = boxes.data[index * 4 + 2].exp() * step;
            let height = boxes.data[index * 4 + 3].exp() * step;
            let mut face = Face {
                x: centre_x - width / 2.0,
                y: centre_y - height / 2.0,
                width,
                height,
                score,
                points: [[0.0; 2]; 5],
            };
            for (point, target) in face.points.iter_mut().enumerate() {
                target[0] = (points.data[index * 10 + point * 2] + column) * step;
                target[1] = (points.data[index * 10 + point * 2 + 1] + row) * step;
            }
            if face.width.is_finite() && face.height.is_finite() {
                faces.push(face);
            }
        }
    }
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for face in faces {
        if kept
            .iter()
            .all(|better| better.overlap(&face) <= OVERLAP_THRESHOLD)
        {
            kept.push(face);
        }
    }
    Ok(kept)
}

/// Rotation, uniform scale and shift: `u = a·x − b·y + tx`, `v = b·x + a·y + ty`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Similarity {
    pub fn forward(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x - self.b * y + self.tx,
            self.b * x + self.a * y + self.ty,
        )
    }

    pub fn inverse(&self, u: f32, v: f32) -> (f32, f32) {
        let scale = (self.a * self.a + self.b * self.b).max(1e-12);
        let dx = u - self.tx;
        let dy = v - self.ty;
        (
            (self.a * dx + self.b * dy) / scale,
            (-self.b * dx + self.a * dy) / scale,
        )
    }
}

/// Least-squares similarity that moves `from` onto `to`.
pub fn fit(from: &[[f32; 2]; 5], to: &[[f32; 2]; 5]) -> Similarity {
    let count = from.len() as f32;
    let mean = |points: &[[f32; 2]; 5], axis: usize| {
        points.iter().map(|point| point[axis]).sum::<f32>() / count
    };
    let (from_x, from_y) = (mean(from, 0), mean(from, 1));
    let (to_x, to_y) = (mean(to, 0), mean(to, 1));
    let mut dot = 0.0;
    let mut cross = 0.0;
    let mut spread = 0.0;
    for (source, target) in from.iter().zip(to) {
        let (x, y) = (source[0] - from_x, source[1] - from_y);
        let (u, v) = (target[0] - to_x, target[1] - to_y);
        dot += x * u + y * v;
        cross += x * v - y * u;
        spread += x * x + y * y;
    }
    let spread = spread.max(1e-12);
    let a = dot / spread;
    let b = cross / spread;
    Similarity {
        a,
        b,
        tx: to_x - (a * from_x - b * from_y),
        ty: to_y - (b * from_x + a * from_y),
    }
}

/// The transform that brings a detected face onto the template of `size`.
pub fn align(points: &[[f32; 2]; 5], size: f32) -> Similarity {
    let factor = size / 512.0;
    let mut template = TEMPLATE;
    for point in &mut template {
        point[0] *= factor;
        point[1] *= factor;
    }
    fit(points, &template)
}

/// Bilinear sample of one channel of interleaved 8-bit pixels.
fn sample_pixels(
    pixels: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    channel: usize,
) -> f32 {
    let x = x.clamp(0.0, (width - 1) as f32);
    let y = y.clamp(0.0, (height - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let at = |px: usize, py: usize| f32::from(pixels[(py * width + px) * 4 + channel]);
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Bilinear sample of one plane of a square float picture.
fn sample_plane(plane: &[f32], side: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (side - 1) as f32);
    let y = y.clamp(0.0, (side - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(side - 1);
    let y1 = (y0 + 1).min(side - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let top = plane[y0 * side + x0] * (1.0 - fx) + plane[y0 * side + x1] * fx;
    let bottom = plane[y1 * side + x0] * (1.0 - fx) + plane[y1 * side + x1] * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Cuts the aligned face out of RGBA pixels: three planes of `side`² values
/// from -1 to 1.
pub fn warp(
    rgba: &[u8],
    width: usize,
    height: usize,
    transform: &Similarity,
    side: usize,
) -> Vec<f32> {
    let mut data = vec![0f32; side * side * 3];
    for v in 0..side {
        for u in 0..side {
            let (x, y) = transform.inverse(u as f32, v as f32);
            for channel in 0..3 {
                data[channel * side * side + v * side + u] =
                    sample_pixels(rgba, width, height, x, y, channel) / 127.5 - 1.0;
            }
        }
    }
    data
}

/// Blends a restored aligned face back into RGBA pixels, fading towards the
/// edge of the aligned square.
pub fn paste(
    rgba: &mut [u8],
    width: usize,
    height: usize,
    transform: &Similarity,
    restored: &[f32],
    side: usize,
    strength: f32,
) {
    let last = (side - 1) as f32;
    let corners = [
        transform.inverse(0.0, 0.0),
        transform.inverse(last, 0.0),
        transform.inverse(0.0, last),
        transform.inverse(last, last),
    ];
    let bound = |pick: fn(f32, f32) -> f32, axis: usize, start: f32| {
        corners.iter().fold(start, |edge, corner| {
            pick(edge, if axis == 0 { corner.0 } else { corner.1 })
        })
    };
    let left = bound(f32::min, 0, f32::INFINITY).floor().max(0.0) as usize;
    let top = bound(f32::min, 1, f32::INFINITY).floor().max(0.0) as usize;
    let right = (bound(f32::max, 0, f32::NEG_INFINITY).ceil().max(0.0) as usize).min(width - 1);
    let bottom = (bound(f32::max, 1, f32::NEG_INFINITY).ceil().max(0.0) as usize).min(height - 1);
    let fade = (side as f32 * FEATHER).max(1.0);
    let plane = side * side;
    for y in top..=bottom {
        for x in left..=right {
            let (u, v) = transform.forward(x as f32, y as f32);
            if u < 0.0 || v < 0.0 || u > last || v > last {
                continue;
            }
            let edge = u.min(v).min(last - u).min(last - v);
            let cover = (edge / fade).clamp(0.0, 1.0) * strength;
            if cover <= 0.0 {
                continue;
            }
            let target = (y * width + x) * 4;
            for channel in 0..3 {
                let new = (sample_plane(
                    &restored[channel * plane..(channel + 1) * plane],
                    side,
                    u,
                    v,
                ) + 1.0)
                    * 127.5;
                let old = f32::from(rgba[target + channel]);
                rgba[target + channel] = (old * (1.0 - cover) + new * cover)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-2
    }

    #[test]
    fn fit_recovers_a_known_transform() {
        let truth = Similarity {
            a: 1.2,
            b: -0.5,
            tx: 30.0,
            ty: -12.0,
        };
        let from = [
            [10.0, 20.0],
            [60.0, 22.0],
            [35.0, 50.0],
            [15.0, 80.0],
            [58.0, 79.0],
        ];
        let mut to = [[0.0; 2]; 5];
        for (target, source) in to.iter_mut().zip(&from) {
            let (u, v) = truth.forward(source[0], source[1]);
            *target = [u, v];
        }
        let found = fit(&from, &to);
        assert!(close(found.a, truth.a) && close(found.b, truth.b));
        assert!(close(found.tx, truth.tx) && close(found.ty, truth.ty));
        let (x, y) = found.inverse(to[2][0], to[2][1]);
        assert!(close(x, from[2][0]) && close(y, from[2][1]));
    }

    #[test]
    fn template_aligns_onto_itself() {
        let transform = align(&TEMPLATE, 512.0);
        assert!(close(transform.a, 1.0) && close(transform.b, 0.0));
        assert!(close(transform.tx, 0.0) && close(transform.ty, 0.0));
        let half = align(&TEMPLATE, 256.0);
        assert!(close(half.a, 0.5));
    }

    fn detector_outputs(size: usize) -> Vec<Tensor> {
        let mut outputs = Vec::new();
        for (kind, per_cell) in [("cls", 1), ("obj", 1), ("bbox", 4), ("kps", 10)] {
            for stride in STRIDES {
                let cells = (size / stride) * (size / stride);
                outputs.push(Tensor {
                    name: format!("{kind}_{stride}"),
                    shape: vec![1, cells as i64, per_cell as i64],
                    data: vec![0.0; cells * per_cell],
                });
            }
        }
        outputs
    }

    #[test]
    fn detections_are_decoded_and_deduplicated() {
        let size = 64;
        let mut outputs = detector_outputs(size);
        // Stride 8 gives an 8×8 grid; mark the cells (2, 3) and (3, 3).
        for (cell, score) in [(3 * 8 + 2, 0.95f32), (3 * 8 + 3, 0.8)] {
            outputs[0].data[cell] = score;
            outputs[3].data[cell] = score;
            outputs[6].data[cell * 4 + 2] = 2.0f32.ln();
            outputs[6].data[cell * 4 + 3] = 2.0f32.ln();
        }
        // The second cell points back at the first face.
        outputs[6].data[(3 * 8 + 3) * 4] = -1.0;
        outputs[9].data[(3 * 8 + 2) * 10] = 0.5;
        let faces = decode(&outputs, size, SCORE_THRESHOLD).unwrap();
        assert_eq!(faces.len(), 1);
        let face = &faces[0];
        assert!(close(face.score, 0.95));
        assert!(close(face.width, 16.0) && close(face.height, 16.0));
        assert!(close(face.x, 8.0) && close(face.y, 16.0));
        assert!(close(face.points[0][0], 20.0) && close(face.points[0][1], 24.0));

        let mut moved = face.clone();
        moved.rescale(2.0);
        assert!(close(moved.x, 16.0) && close(moved.points[0][0], 40.0));
        assert!(decode(&outputs[..4], size, SCORE_THRESHOLD).is_err());
    }

    #[test]
    fn warp_and_paste_round_trip() {
        let (width, height) = (32usize, 32usize);
        let mut rgba = vec![0u8; width * height * 4];
        for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
            let x = index % width;
            pixel.copy_from_slice(&[(x * 8) as u8, 100, 200, 255]);
        }
        let identity = Similarity {
            a: 1.0,
            b: 0.0,
            tx: 0.0,
            ty: 0.0,
        };
        let side = 32;
        let aligned = warp(&rgba, width, height, &identity, side);
        assert!(close(aligned[5], (5.0 * 8.0) / 127.5 - 1.0));
        // Pasting a white face at full strength changes the centre only.
        let white = vec![1.0f32; side * side * 3];
        let before = rgba.clone();
        paste(&mut rgba, width, height, &identity, &white, side, 1.0);
        let centre = (16 * width + 16) * 4;
        assert_eq!(&rgba[centre..centre + 3], &[255, 255, 255]);
        assert_eq!(&rgba[..3], &before[..3]);
        assert_eq!(rgba[centre + 3], 255);
    }
}
