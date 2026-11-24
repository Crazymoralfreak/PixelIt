//! Collecting the image files behind the paths a user picked.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pixelit_core::ImageFormat;
use walkdir::WalkDir;

use crate::process::Item;

/// Whether the file has an extension of a format PixelIt reads.
pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(ImageFormat::from_extension)
        .is_some()
}

/// Image files of a folder. `skip` names a subfolder left out, usually the
/// one results are written into.
pub fn folder_files(folder: &Path, recursive: bool, skip: Option<&str>) -> Vec<PathBuf> {
    let depth = if recursive { usize::MAX } else { 1 };
    let mut files: Vec<PathBuf> = WalkDir::new(folder)
        .max_depth(depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let hidden_output = entry.depth() > 0
                && entry.file_type().is_dir()
                && skip.is_some_and(|skip| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(skip)
                });
            !hidden_output
        })
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file() && is_image(entry.path()))
        .map(walkdir::DirEntry::into_path)
        .collect();
    files.sort_by_cached_key(|path| path.to_string_lossy().to_lowercase());
    files
}

/// Turns picked files and folders into batch items, numbered from 1, without
/// repeats. A folder becomes the root of the files found in it.
pub fn collect(inputs: &[PathBuf], recursive: bool, skip: Option<&str>) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut add = |source: PathBuf, root: Option<&Path>, items: &mut Vec<Item>| {
        if seen.insert(source.clone()) {
            items.push(Item {
                source,
                root: root.map(Path::to_path_buf),
                index: items.len() as u64 + 1,
                ..Item::default()
            });
        }
    };
    for input in inputs {
        if input.is_dir() {
            for file in folder_files(input, recursive, skip) {
                add(file, Some(input), &mut items);
            }
        } else if input.is_file() {
            add(input.clone(), None, &mut items);
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn collects_images_and_skips_the_output_folder() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(root.join("PixelIt")).unwrap();
        for name in ["b.JPG", "a.png", "notes.txt", "sub/c.webp", "PixelIt/a.png"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        let items = collect(&[root.to_path_buf()], true, Some("pixelit"));
        let names: Vec<String> = items
            .iter()
            .map(|item| {
                item.source
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, ["a.png", "b.JPG", "sub/c.webp"]);
        assert_eq!(items[2].index, 3);
        assert_eq!(items[0].root.as_deref(), Some(root));

        let flat = collect(&[root.to_path_buf()], false, Some("PixelIt"));
        assert_eq!(flat.len(), 2);

        let picked = collect(
            &[
                root.join("a.png"),
                root.join("a.png"),
                root.join("notes.txt"),
            ],
            true,
            None,
        );
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].root, None);
    }

    #[test]
    fn recognises_extensions() {
        assert!(is_image(Path::new("a.HEIC")));
        assert!(is_image(Path::new("a.cr2")));
        assert!(!is_image(Path::new("a.txt")));
        assert!(!is_image(Path::new("a")));
    }
}
