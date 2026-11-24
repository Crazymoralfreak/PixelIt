//! Local models: background removal, upscaling, object removal and face
//! restoration. All of them run in the worker process through ONNX Runtime.

use std::sync::{Mutex, PoisonError};

use pixelit_core::pipeline::{Mask, ResizeFilter};
use pixelit_core::{ops, CancelToken, Error, Image, Limits, Result};
use pixelit_engine::Paths;

use crate::catalog::{self, Entry, Spec};
use crate::faces::{self, Face};
use crate::install;
use crate::worker::{self, ModelInfo, Tensor, Worker};

/// Tile side for upscaling models that accept any input size.
const FREE_TILE: usize = 256;

/// Faces smaller than this are left alone: there is nothing to restore from.
const MIN_FACE: f32 = 24.0;

/// Most faces restored in one picture.
const MAX_FACES: usize = 24;

/// Where sessions run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub gpu: bool,
    pub gpu_index: i32,
    pub threads: usize,
}

pub struct Onnx {
    paths: Paths,
    device: Mutex<Device>,
    worker: Mutex<Option<Worker>>,
}

fn wrong_model(entry: &Entry) -> Error {
    Error::new("modelMissing").with("model", entry.id)
}

fn failed(text: &str) -> Error {
    Error::new("aiModelFailed").detail(text)
}

/// Converts interleaved 8-bit pixels into planes of floats.
/// `order` lists the source channel of every plane.
fn planes(
    pixels: &[u8],
    channels: usize,
    order: [usize; 3],
    map: impl Fn(usize, f32) -> f32,
) -> Vec<f32> {
    let count = pixels.len() / channels;
    let mut data = Vec::with_capacity(count * 3);
    for (plane, channel) in order.into_iter().enumerate() {
        data.extend(
            pixels
                .chunks_exact(channels)
                .map(|pixel| map(plane, f32::from(pixel[channel]))),
        );
    }
    data
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Builds an opaque picture out of planes with values from 0 to 1.
fn from_planes(data: &[f32], width: usize, height: usize) -> Result<Image> {
    let count = width * height;
    if data.len() < count * 3 {
        return Err(failed("the model returned too little data"));
    }
    let mut rgba = vec![255u8; count * 4];
    for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        pixel[0] = to_byte(data[index]);
        pixel[1] = to_byte(data[count + index]);
        pixel[2] = to_byte(data[2 * count + index]);
    }
    Image::from_rgba8(width as u32, height as u32, rgba)
}

/// A single-channel plane as a grey picture, for resizing.
fn gray_image(values: &[u8], width: u32, height: u32) -> Result<Image> {
    let mut rgba = Vec::with_capacity(values.len() * 4);
    for value in values {
        rgba.extend_from_slice(&[*value, *value, *value, 255]);
    }
    Image::from_rgba8(width, height, rgba)
}

fn gray_values(image: &Image) -> Vec<u8> {
    image
        .to_rgba8()
        .chunks_exact(4)
        .map(|pixel| pixel[0])
        .collect()
}

fn first_output(outputs: Vec<Tensor>) -> Result<Tensor> {
    outputs
        .into_iter()
        .next()
        .ok_or_else(|| failed("the model returned nothing"))
}

fn input_name(info: &ModelInfo, index: usize) -> Result<&str> {
    info.inputs
        .get(index)
        .map(|port| port.name.as_str())
        .ok_or_else(|| failed("the model has too few inputs"))
}

/// The pixel box around the painted part of a mask: `[left, top, right, bottom)`.
fn mask_bounds(mask: &Mask) -> Option<[u32; 4]> {
    let width = mask.width as usize;
    let mut bounds: Option<[u32; 4]> = None;
    for (index, value) in mask.data.iter().enumerate() {
        if *value > 127 {
            let x = (index % width) as u32;
            let y = (index / width) as u32;
            bounds = Some(match bounds {
                None => [x, y, x + 1, y + 1],
                Some([left, top, right, bottom]) => {
                    [left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1)]
                }
            });
        }
    }
    bounds
}

/// Grows the painted box so the model sees enough of the surroundings, and
/// evens out its sides where the picture allows.
fn context_box(bounds: [u32; 4], width: u32, height: u32) -> [u32; 4] {
    let [left, top, right, bottom] = bounds;
    let box_width = right - left;
    let box_height = bottom - top;
    let side = box_width.max(box_height);
    let margin = side / 2 + 32;
    let wanted = side + 2 * margin;
    let grow = |start: u32, end: u32, limit: u32| -> (u32, u32) {
        let size = wanted.min(limit);
        let centre = u32::midpoint(start, end);
        let begin = centre.saturating_sub(size / 2).min(limit - size);
        (begin, begin + size)
    };
    let (x0, x1) = grow(left, right, width);
    let (y0, y1) = grow(top, bottom, height);
    [x0, y0, x1, y1]
}

fn crop_rgba(rgba: &[u8], width: u32, region: [u32; 4]) -> Vec<u8> {
    let [x0, y0, x1, y1] = region;
    let mut out = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
    for y in y0..y1 {
        let start = ((y * width + x0) * 4) as usize;
        let end = ((y * width + x1) * 4) as usize;
        out.extend_from_slice(&rgba[start..end]);
    }
    out
}

impl Onnx {
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            device: Mutex::new(Device::default()),
            worker: Mutex::new(None),
        }
    }

    /// Chooses where sessions run; a change restarts the worker.
    pub fn set_device(&self, device: Device) {
        let mut current = self.device.lock().unwrap_or_else(PoisonError::into_inner);
        if *current != device {
            *current = device;
            drop(current);
            self.stop();
        }
    }

    /// Stops the worker and frees the memory of its models.
    pub fn stop(&self) {
        *self.worker.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Runs `task` with the worker; models other than `keep` are closed first
    /// so memory holds only what the task needs.
    fn session<T>(
        &self,
        keep: &[&Entry],
        task: impl FnOnce(&mut Worker, &[ModelInfo]) -> Result<T>,
    ) -> Result<T> {
        let device = *self.device.lock().unwrap_or_else(PoisonError::into_inner);
        let mut guard = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.as_mut().is_some_and(|worker| !worker.is_alive()) {
            *guard = None;
        }
        if guard.is_none() {
            let runtime = install::main_path(&self.paths, catalog::RUNTIME)?;
            let program = worker::worker_program()?;
            let log = self.paths.logs().join("worker.log");
            *guard = Some(Worker::start(&program, &runtime, Some(&log))?);
        }
        let Some(worker) = guard.as_mut() else {
            return Err(Error::new("aiWorkerFailed"));
        };
        let result = (|| -> Result<T> {
            for id in worker.loaded() {
                if !keep.iter().any(|entry| entry.id == id) {
                    worker.unload(&id);
                }
            }
            let mut infos = Vec::with_capacity(keep.len());
            for entry in keep {
                let path = install::main_path(&self.paths, entry.id)?;
                infos.push(worker.load(
                    entry.id,
                    &path,
                    device.gpu,
                    device.gpu_index,
                    device.threads,
                )?);
            }
            task(worker, &infos)
        })();
        if result
            .as_ref()
            .is_err_and(|error| error.code == "aiWorkerFailed")
        {
            // The process is gone or out of step; the next task starts a new one.
            *guard = None;
        }
        result
    }

    /// Makes the background transparent.
    pub fn remove_background(
        &self,
        image: &Image,
        entry: &Entry,
        cancel: &CancelToken,
    ) -> Result<Image> {
        let Spec::Matte { size, mean, std } = entry.spec else {
            return Err(wrong_model(entry));
        };
        cancel.check()?;
        let width = image.width();
        let height = image.height();
        let small = ops::resize::resize_to(image, size, size, ResizeFilter::Lanczos3, false, None)?
            .to_rgb8();
        let peak = f32::from(small.iter().copied().max().unwrap_or(255).max(1));
        let data = planes(&small, 3, [0, 1, 2], |plane, value| {
            (value / peak - mean[plane]) / std[plane]
        });
        let side = size as usize;
        let matte = self.session(&[entry], |worker, infos| {
            let input = Tensor::new(input_name(&infos[0], 0)?, &[1, 3, side, side], data);
            first_output(worker.run(entry.id, &[input])?)
        })?;
        cancel.check()?;
        let count = side * side;
        let values = matte
            .data
            .get(..count)
            .ok_or_else(|| failed("the model returned too little data"))?;
        let low = values.iter().copied().fold(f32::INFINITY, f32::min);
        let high = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let range = (high - low).max(1e-6);
        let small_matte: Vec<u8> = values
            .iter()
            .map(|value| to_byte((value - low) / range))
            .collect();
        let full = ops::resize::resize_to(
            &gray_image(&small_matte, size, size)?,
            width,
            height,
            ResizeFilter::Bilinear,
            false,
            None,
        )?;
        let matte = gray_values(&full);
        let mut rgba = image.to_rgba8();
        for (pixel, cover) in rgba.chunks_exact_mut(4).zip(&matte) {
            pixel[3] = ((u32::from(pixel[3]) * u32::from(*cover) + 127) / 255) as u8;
        }
        Ok(Image::from_rgba8(width, height, rgba)?.into_depth(image.depth()))
    }

    /// Enlarges the picture `factor` times, tile by tile.
    pub fn upscale(
        &self,
        image: &Image,
        entry: &Entry,
        factor: u32,
        cancel: &CancelToken,
    ) -> Result<Image> {
        if entry.spec != Spec::Upscale {
            return Err(wrong_model(entry));
        }
        let width = image.width() as usize;
        let height = image.height() as usize;
        Limits::default().check_size(image.width() * factor, image.height() * factor)?;
        let rgb = image.to_rgb8();
        let enlarged = self.session(&[entry], |worker, infos| {
            let info = &infos[0];
            let name = input_name(info, 0)?.to_owned();
            let tile = info.inputs[0]
                .fixed(2)
                .or(info.inputs[0].fixed(3))
                .unwrap_or(FREE_TILE);
            let pad = (tile / 8).clamp(4, 16);
            let step = tile - 2 * pad;
            let mut scale = 0usize;
            let mut out: Vec<u8> = Vec::new();
            for tile_y in 0..height.div_ceil(step) {
                for tile_x in 0..width.div_ceil(step) {
                    cancel.check()?;
                    let left = (tile_x * step) as i64 - pad as i64;
                    let top = (tile_y * step) as i64 - pad as i64;
                    let mut data = vec![0f32; tile * tile * 3];
                    for y in 0..tile {
                        let source_y = (top + y as i64).clamp(0, height as i64 - 1) as usize;
                        for x in 0..tile {
                            let source_x = (left + x as i64).clamp(0, width as i64 - 1) as usize;
                            let pixel = (source_y * width + source_x) * 3;
                            for channel in 0..3 {
                                data[channel * tile * tile + y * tile + x] =
                                    f32::from(rgb[pixel + channel]) / 255.0;
                            }
                        }
                    }
                    let input = Tensor::new(&name, &[1, 3, tile, tile], data);
                    let result = first_output(worker.run(entry.id, &[input])?)?;
                    if scale == 0 {
                        scale = result.side(3) / tile;
                        if !(1..=8).contains(&scale) || result.side(2) != tile * scale {
                            return Err(failed("unexpected output size"));
                        }
                        Limits::default()
                            .check_size((width * scale) as u32, (height * scale) as u32)?;
                        out = vec![255u8; width * scale * height * scale * 4];
                    }
                    let big = tile * scale;
                    if result.data.len() < big * big * 3 {
                        return Err(failed("the model returned too little data"));
                    }
                    let x_end = ((tile_x + 1) * step).min(width) * scale;
                    let y_end = ((tile_y + 1) * step).min(height) * scale;
                    for y in tile_y * step * scale..y_end {
                        let inner_y = y - tile_y * step * scale + pad * scale;
                        for x in tile_x * step * scale..x_end {
                            let inner_x = x - tile_x * step * scale + pad * scale;
                            let target = (y * width * scale + x) * 4;
                            for channel in 0..3 {
                                out[target + channel] = to_byte(
                                    result.data[channel * big * big + inner_y * big + inner_x],
                                );
                            }
                        }
                    }
                }
            }
            Image::from_rgba8((width * scale) as u32, (height * scale) as u32, out)
        })?;
        cancel.check()?;
        let target_width = image.width() * factor;
        let target_height = image.height() * factor;
        let mut result = ops::resize::resize_to(
            &enlarged,
            target_width,
            target_height,
            ResizeFilter::Lanczos3,
            false,
            None,
        )?;
        if image.has_alpha() {
            // Transparency is enlarged with an ordinary filter.
            let alpha = ops::resize::resize_to(
                image,
                target_width,
                target_height,
                ResizeFilter::Lanczos3,
                false,
                None,
            )?
            .to_rgba8();
            let mut rgba = result.to_rgba8();
            for (pixel, source) in rgba.chunks_exact_mut(4).zip(alpha.chunks_exact(4)) {
                pixel[3] = source[3];
            }
            result = Image::from_rgba8(target_width, target_height, rgba)?;
        } else {
            result.set_has_alpha(false);
        }
        Ok(result.into_depth(image.depth()))
    }

    /// Repaints the masked area from its surroundings.
    pub fn inpaint(
        &self,
        image: &Image,
        mask: &Mask,
        entry: &Entry,
        cancel: &CancelToken,
    ) -> Result<Image> {
        let Spec::Inpaint { size } = entry.spec else {
            return Err(wrong_model(entry));
        };
        let width = image.width();
        let height = image.height();
        if mask.width != width || mask.height != height {
            return Err(failed("the mask and the picture differ in size"));
        }
        let Some(bounds) = mask_bounds(mask) else {
            return Ok(image.clone());
        };
        cancel.check()?;
        let region = context_box(bounds, width, height);
        let [x0, y0, x1, y1] = region;
        let crop_width = x1 - x0;
        let crop_height = y1 - y0;
        let mut rgba = image.to_rgba8();
        let crop = Image::from_rgba8(crop_width, crop_height, crop_rgba(&rgba, width, region))?;
        let small = ops::resize::resize_to(&crop, size, size, ResizeFilter::Lanczos3, false, None)?
            .to_rgb8();
        let mut mask_crop = Vec::with_capacity((crop_width * crop_height) as usize);
        for y in y0..y1 {
            let start = (y * width + x0) as usize;
            mask_crop.extend_from_slice(&mask.data[start..start + crop_width as usize]);
        }
        let small_mask = gray_values(&ops::resize::resize_to(
            &gray_image(&mask_crop, crop_width, crop_height)?,
            size,
            size,
            ResizeFilter::Bilinear,
            false,
            None,
        )?);
        let side = size as usize;
        let picture = planes(&small, 3, [0, 1, 2], |_, value| value / 255.0);
        // Any touched pixel counts, which widens the mask by about a pixel.
        let holes: Vec<f32> = small_mask
            .iter()
            .map(|value| if *value > 0 { 1.0 } else { 0.0 })
            .collect();
        let filled = self.session(&[entry], |worker, infos| {
            let info = &infos[0];
            let first_is_mask = input_name(info, 0)?.to_ascii_lowercase().contains("mask");
            let (picture_index, mask_index) = if first_is_mask { (1, 0) } else { (0, 1) };
            let inputs = [
                Tensor::new(
                    input_name(info, picture_index)?,
                    &[1, 3, side, side],
                    picture,
                ),
                Tensor::new(input_name(info, mask_index)?, &[1, 1, side, side], holes),
            ];
            first_output(worker.run(entry.id, &inputs)?)
        })?;
        cancel.check()?;
        // Some exports answer in 0..255, others in 0..1.
        let peak = filled.data.iter().copied().fold(0.0f32, f32::max);
        let divisor = if peak > 2.0 { 255.0 } else { 1.0 };
        let normalized: Vec<f32> = filled.data.iter().map(|value| value / divisor).collect();
        let patch = ops::resize::resize_to(
            &from_planes(&normalized, side, side)?,
            crop_width,
            crop_height,
            ResizeFilter::Lanczos3,
            false,
            None,
        )?
        .to_rgba8();
        let soft = gray_values(&ops::resize::resize_to(
            &gray_image(&small_mask, size, size)?,
            crop_width,
            crop_height,
            ResizeFilter::Bilinear,
            false,
            None,
        )?);
        for y in 0..crop_height {
            for x in 0..crop_width {
                let local = (y * crop_width + x) as usize;
                let cover = u32::from(soft[local].max(mask_crop[local]));
                if cover == 0 {
                    continue;
                }
                let target = (((y0 + y) * width + x0 + x) * 4) as usize;
                for channel in 0..3 {
                    let old = u32::from(rgba[target + channel]);
                    let new = u32::from(patch[local * 4 + channel]);
                    rgba[target + channel] =
                        ((old * (255 - cover) + new * cover + 127) / 255) as u8;
                }
            }
        }
        let mut result = Image::from_rgba8(width, height, rgba)?;
        result.set_has_alpha(image.has_alpha());
        Ok(result.into_depth(image.depth()))
    }

    /// Finds faces in a picture.
    pub fn detect_faces(
        &self,
        image: &Image,
        keep: &[&Entry],
        cancel: &CancelToken,
    ) -> Result<Vec<Face>> {
        let detector =
            catalog::find(catalog::DETECTOR).ok_or_else(|| Error::new("modelMissing"))?;
        let Spec::Detector { size } = detector.spec else {
            return Err(wrong_model(detector));
        };
        cancel.check()?;
        let side = size as usize;
        let longest = image.width().max(image.height()) as f32;
        let scale = (size as f32 / longest).min(2.0);
        let small_width = ((image.width() as f32 * scale).round() as u32).clamp(1, size);
        let small_height = ((image.height() as f32 * scale).round() as u32).clamp(1, size);
        let small = ops::resize::resize_to(
            image,
            small_width,
            small_height,
            ResizeFilter::Bilinear,
            false,
            None,
        )?
        .to_rgb8();
        // The detector takes blue-green-red planes with values from 0 to 255.
        let mut data = vec![0f32; side * side * 3];
        for y in 0..small_height as usize {
            for x in 0..small_width as usize {
                let pixel = (y * small_width as usize + x) * 3;
                for plane in 0..3 {
                    data[plane * side * side + y * side + x] = f32::from(small[pixel + 2 - plane]);
                }
            }
        }
        let mut models: Vec<&Entry> = vec![detector];
        models.extend_from_slice(keep);
        let outputs = self.session(&models, |worker, infos| {
            let input = Tensor::new(input_name(&infos[0], 0)?, &[1, 3, side, side], data);
            worker.run(detector.id, &[input])
        })?;
        let mut found = faces::decode(&outputs, side, faces::SCORE_THRESHOLD)?;
        for face in &mut found {
            face.rescale(1.0 / scale);
        }
        Ok(found)
    }

    /// Restores the faces of a picture and blends them in with `strength`
    /// from 0 to 1.
    pub fn restore_faces(
        &self,
        image: &Image,
        entry: &Entry,
        strength: f32,
        cancel: &CancelToken,
    ) -> Result<Image> {
        let Spec::Faces { size } = entry.spec else {
            return Err(wrong_model(entry));
        };
        let strength = strength.clamp(0.0, 1.0);
        if strength <= 0.0 {
            return Ok(image.clone());
        }
        let mut found = self.detect_faces(image, &[entry], cancel)?;
        found.retain(|face| face.width.min(face.height) >= MIN_FACE);
        found.truncate(MAX_FACES);
        if found.is_empty() {
            return Ok(image.clone());
        }
        let width = image.width() as usize;
        let height = image.height() as usize;
        let side = size as usize;
        let mut rgba = image.to_rgba8();
        let detector =
            catalog::find(catalog::DETECTOR).ok_or_else(|| Error::new("modelMissing"))?;
        for face in &found {
            cancel.check()?;
            let transform = faces::align(&face.points, size as f32);
            let aligned = faces::warp(&rgba, width, height, &transform, side);
            let restored = self.session(&[detector, entry], |worker, infos| {
                let input = Tensor::new(input_name(&infos[1], 0)?, &[1, 3, side, side], aligned);
                first_output(worker.run(entry.id, &[input])?)
            })?;
            if restored.data.len() < side * side * 3 {
                return Err(failed("the model returned too little data"));
            }
            faces::paste(
                &mut rgba,
                width,
                height,
                &transform,
                &restored.data,
                side,
                strength,
            );
        }
        let mut result = Image::from_rgba8(image.width(), image.height(), rgba)?;
        result.set_has_alpha(image.has_alpha());
        Ok(result.into_depth(image.depth()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planes_follow_the_channel_order() {
        let pixels = [10u8, 20, 30, 40, 50, 60];
        let rgb = planes(&pixels, 3, [0, 1, 2], |_, value| value);
        assert_eq!(rgb, [10.0, 40.0, 20.0, 50.0, 30.0, 60.0]);
        let bgr = planes(&pixels, 3, [2, 1, 0], |plane, value| value + plane as f32);
        assert_eq!(bgr, [30.0, 60.0, 21.0, 51.0, 12.0, 42.0]);
    }

    #[test]
    fn planes_become_pictures() {
        let data = [1.0, 0.0, 0.5, 2.0, 0.0, -1.0];
        let image = from_planes(&data, 2, 1).unwrap();
        assert_eq!(image.pixel8(0, 0), [255, 128, 0, 255]);
        assert_eq!(image.pixel8(1, 0), [0, 255, 0, 255]);
        assert!(from_planes(&data, 2, 2).is_err());
    }

    #[test]
    fn mask_bounds_cover_the_painted_pixels() {
        let mut mask = Mask {
            width: 8,
            height: 6,
            data: vec![0; 48],
        };
        assert_eq!(mask_bounds(&mask), None);
        mask.data[8 + 2] = 255;
        mask.data[4 * 8 + 5] = 200;
        mask.data[3] = 100;
        assert_eq!(mask_bounds(&mask), Some([2, 1, 6, 5]));
    }

    #[test]
    fn context_stays_inside_the_picture() {
        let region = context_box([10, 10, 20, 14], 1000, 800);
        assert_eq!(region[2] - region[0], region[3] - region[1]);
        assert!(region[0] < 10 && region[2] > 20);
        let tight = context_box([0, 0, 50, 30], 60, 40);
        assert_eq!(tight, [0, 0, 60, 40]);
        let corner = context_box([990, 790, 1000, 800], 1000, 800);
        assert_eq!(corner[2], 1000);
        assert_eq!(corner[3], 800);
    }

    #[test]
    fn crops_copy_rows() {
        let rgba: Vec<u8> = (0..4 * 3 * 4).map(|value| value as u8).collect();
        let crop = crop_rgba(&rgba, 4, [1, 1, 3, 3]);
        assert_eq!(crop.len(), 16);
        assert_eq!(&crop[..4], &rgba[20..24]);
        assert_eq!(&crop[12..], &rgba[40..44]);
    }

    #[test]
    fn tasks_fail_cleanly_without_the_runtime() {
        let folder = tempfile::tempdir().unwrap();
        let onnx = Onnx::new(Paths::at(folder.path()));
        let image =
            Image::new(8, 8, pixelit_core::Depth::Eight, pixelit_core::Color::WHITE).unwrap();
        let entry = catalog::find("u2netp").unwrap();
        let error = onnx
            .remove_background(&image, entry, &CancelToken::new())
            .unwrap_err();
        assert_eq!(error.code, "componentMissing");
        let wrong = onnx
            .upscale(&image, entry, 2, &CancelToken::new())
            .unwrap_err();
        assert_eq!(wrong.code, "modelMissing");
    }
}
