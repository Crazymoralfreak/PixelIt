//! Finding duplicate and near-duplicate pictures.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pixelit_core::hash::{hamming, perceptual_hash};
use pixelit_core::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::jobs::JobContext;
use crate::process::Runner;
use crate::thumbs::load_preview;

/// Side of the reduced picture the perceptual hash is taken from.
const HASH_SIDE: u32 = 256;

/// Largest accepted distance: beyond it pictures no longer look alike.
pub const MAX_DISTANCE: u32 = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// Hamming distance to the first file of the group.
    pub distance: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    /// Every file of the group has the same bytes.
    pub exact: bool,
    /// Files of the group, the largest picture first.
    pub files: Vec<DuplicateFile>,
}

struct Fingerprint {
    path: PathBuf,
    bytes: u64,
    width: u32,
    height: u32,
    content: [u8; 32],
    look: u64,
}

fn fingerprint(runner: &Runner, path: &Path, job: &JobContext) -> Result<Fingerprint> {
    job.checkpoint()?;
    let data = fs::read(path)?;
    let content = *blake3::hash(&data).as_bytes();
    let bytes = data.len() as u64;
    drop(data);
    let preview = load_preview(runner, path, HASH_SIDE, 0, &job.cancel)?;
    Ok(Fingerprint {
        path: path.to_path_buf(),
        bytes,
        width: preview.info.width,
        height: preview.info.height,
        content,
        look: perceptual_hash(&preview.image)?,
    })
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn group(prints: &[Fingerprint], distance: u32) -> Vec<DuplicateGroup> {
    let mut parents: Vec<usize> = (0..prints.len()).collect();
    for first in 0..prints.len() {
        for second in first + 1..prints.len() {
            let alike = prints[first].content == prints[second].content
                || hamming(prints[first].look, prints[second].look) <= distance;
            if alike {
                let (a, b) = (root(&mut parents, first), root(&mut parents, second));
                if a != b {
                    parents[b] = a;
                }
            }
        }
    }
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); prints.len()];
    for index in 0..prints.len() {
        let top = root(&mut parents, index);
        members[top].push(index);
    }
    let mut groups: Vec<DuplicateGroup> = members
        .into_iter()
        .filter(|indexes| indexes.len() > 1)
        .map(|mut indexes| {
            indexes.sort_by_key(|index| {
                let print = &prints[*index];
                std::cmp::Reverse((
                    u64::from(print.width) * u64::from(print.height),
                    print.bytes,
                ))
            });
            let best = &prints[indexes[0]];
            DuplicateGroup {
                exact: indexes
                    .iter()
                    .all(|index| prints[*index].content == best.content),
                files: indexes
                    .iter()
                    .map(|index| {
                        let print = &prints[*index];
                        DuplicateFile {
                            path: print.path.clone(),
                            bytes: print.bytes,
                            width: print.width,
                            height: print.height,
                            distance: hamming(best.look, print.look),
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    groups.sort_by(|a, b| a.files[0].path.cmp(&b.files[0].path));
    groups
}

/// Groups files that hold the same or nearly the same picture. `distance`
/// is the tolerance from `0` (identical look) to [`MAX_DISTANCE`]. Files that
/// cannot be read are left out.
pub fn find(
    runner: &Runner,
    files: &[PathBuf],
    distance: u32,
    job: &JobContext,
) -> Result<Vec<DuplicateGroup>> {
    let total = files.len() as u64;
    let finished = AtomicU64::new(0);
    job.progress(0, total, "");
    let prints: Vec<Fingerprint> = files
        .par_iter()
        .filter_map(|path| {
            let print = fingerprint(runner, path, job).ok();
            let done = finished.fetch_add(1, Ordering::Relaxed) + 1;
            let name = path
                .file_name()
                .map_or_default(|name| name.to_string_lossy().into_owned());
            job.progress(done, total, &name);
            print
        })
        .collect();
    job.cancel.check()?;
    Ok(group(&prints, distance.min(MAX_DISTANCE)))
}

#[cfg(test)]
mod tests {
    use pixelit_codecs::{encode, EncodeOptions};
    use pixelit_core::{CancelToken, Image, ImageFormat};

    use super::*;

    /// A smooth picture without repetition, like a photograph. The look of an
    /// exactly periodic pattern has almost no low frequencies, and its hash
    /// would be decided by rounding.
    fn picture(seed: u32) -> Image {
        let (width, height) = (96u32, 64u32);
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let (fx, fy) = (x as f32, y as f32);
                let wave = if seed == 0 {
                    (fx / 17.0).sin() + (fy / 11.0).cos()
                } else {
                    (fx / 7.0).cos() - (fy / 23.0).sin()
                };
                let spot = (-((fx - 30.0).powi(2) + (fy - 20.0).powi(2)) / 300.0).exp();
                let value = (128.0 + wave * 45.0 + spot * 35.0).clamp(0.0, 255.0) as u8;
                data.extend_from_slice(&[value, value / 2 + (x as u8), 255 - value, 255]);
            }
        }
        Image::from_rgba8(width, height, data).unwrap()
    }

    fn write(path: &Path, image: &Image, format: ImageFormat, quality: u8) {
        let options = EncodeOptions::new(format).with_quality(quality);
        fs::write(path, encode(image, &options, &CancelToken::new()).unwrap()).unwrap();
    }

    #[test]
    fn finds_copies_and_recompressions() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        let first = picture(0);
        write(&root.join("a.png"), &first, ImageFormat::Png, 90);
        fs::copy(root.join("a.png"), root.join("a-copy.png")).unwrap();
        write(&root.join("a.jpg"), &first, ImageFormat::Jpeg, 85);
        write(&root.join("other.png"), &picture(1), ImageFormat::Png, 90);
        fs::write(root.join("broken.png"), b"nope").unwrap();

        let files: Vec<PathBuf> = ["a.png", "a-copy.png", "a.jpg", "other.png", "broken.png"]
            .iter()
            .map(|name| root.join(name))
            .collect();
        let groups = find(&Runner::default(), &files, 6, &JobContext::detached()).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 3);
        assert!(!groups[0].exact);

        let exact = find(&Runner::default(), &files[..2], 0, &JobContext::detached()).unwrap();
        assert_eq!(exact.len(), 1);
        assert!(exact[0].exact);
    }
}
