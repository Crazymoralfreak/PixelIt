//! Catalogue of downloadable components and models.
//!
//! Every entry pins an exact file: address, size and hash. The hashes were
//! taken from the publishers' own indexes (the NuGet catalogue, the Hugging
//! Face LFS index, GitHub release digests); a download that does not match is
//! deleted. Nothing is downloaded until the user asks for it.

use serde::Serialize;

use crate::net::Checksum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// ONNX Runtime (DirectML on Windows, Core ML on macOS): needed by every local model.
    Runtime,
    /// PDFium: opens PDF files.
    Pdf,
    /// stable-diffusion.cpp: built-in image generation.
    Generator,
    Background,
    Upscale,
    Inpaint,
    Faces,
    /// Helper models that other models need, hidden from the task lists.
    Detector,
}

impl Kind {
    /// Components live in `components`, models in `models`.
    pub fn is_component(self) -> bool {
        matches!(self, Self::Runtime | Self::Pdf | Self::Generator)
    }
}

/// What to keep from a downloaded file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unpack {
    /// The file itself, stored under this name.
    File(&'static str),
    /// Named files of a zip archive: `(name inside, name on disk)`.
    Entries(&'static [(&'static str, &'static str)]),
    /// The whole zip archive.
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Download {
    pub url: &'static str,
    pub bytes: u64,
    pub checksum: Checksum,
    pub unpack: Unpack,
}

/// How pictures are prepared for a model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spec {
    None,
    /// Salient-object matting: the picture is squeezed to `size`, normalised
    /// with `mean` and `std`, and the first output is the matte.
    Matte {
        size: u32,
        mean: [f32; 3],
        std: [f32; 3],
    },
    /// Super-resolution on tiles; the tile size comes from the model.
    Upscale,
    /// LaMa: picture and mask of `size`.
    Inpaint {
        size: u32,
    },
    /// GFPGAN: an aligned face of `size`, values from -1 to 1.
    Faces {
        size: u32,
    },
    /// YuNet face detector with an input of `size`.
    Detector {
        size: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub id: &'static str,
    pub kind: Kind,
    /// Proper name of the component or model; shown as is.
    pub name: &'static str,
    pub license: &'static str,
    /// Home page of the project.
    pub source: &'static str,
    pub downloads: &'static [Download],
    /// Entries that must be installed first.
    pub requires: &'static [&'static str],
    pub spec: Spec,
}

impl Entry {
    /// Total download size.
    pub fn bytes(&self) -> u64 {
        self.downloads.iter().map(|download| download.bytes).sum()
    }

    /// Whether this entry has a download for this computer.
    pub fn is_offered(&self) -> bool {
        !self.downloads.is_empty()
    }

    /// Whether this entry and everything it needs can be installed here.
    pub fn is_available(&self) -> bool {
        self.is_offered()
            && self.requires.iter().all(|required| {
                ENTRIES
                    .iter()
                    .find(|entry| entry.id == *required)
                    .is_some_and(Entry::is_offered)
            })
    }

    /// Name of the main file: the model, the library or the program.
    pub fn main_file(&self) -> &'static str {
        match self.kind {
            Kind::Generator => {
                if cfg!(windows) {
                    "sd-cli.exe"
                } else {
                    "sd-cli"
                }
            }
            Kind::Runtime
            | Kind::Pdf
            | Kind::Background
            | Kind::Upscale
            | Kind::Inpaint
            | Kind::Faces
            | Kind::Detector => match self.downloads.first().map(|download| download.unpack) {
                Some(Unpack::File(name)) => name,
                Some(Unpack::Entries(entries)) => entries.first().map_or("", |entry| entry.1),
                Some(Unpack::All) | None => "",
            },
        }
    }
}

pub const RUNTIME: &str = "onnxruntime";
pub const PDFIUM: &str = "pdfium";
pub const DETECTOR: &str = "yunet";

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];
const UNIT_STD: [f32; 3] = [1.0, 1.0, 1.0];

const NEEDS_RUNTIME: &[&str] = &[RUNTIME];

#[cfg(windows)]
const RUNTIME_NAME: &str = "ONNX Runtime 1.24.4 + DirectML 1.15.4";
#[cfg(windows)]
const RUNTIME_LICENSE: &str = "MIT; DirectML redistributable licence";
#[cfg(not(windows))]
const RUNTIME_NAME: &str = "ONNX Runtime 1.24.4";
#[cfg(not(windows))]
const RUNTIME_LICENSE: &str = "MIT";

#[cfg(windows)]
const RUNTIME_DOWNLOADS: &[Download] = &[
    Download {
        url: "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg",
        bytes: 12_458_649,
        checksum: Checksum::Sha512(
            "ZjOyv495vhfVXoTJp2utRyn8ir1TFIvCj0B9JP8QZGBXS0oFx+lY7ZCZqSdnVShQX6EtxYi2T3VOtYXcmBTV4A==",
        ),
        unpack: Unpack::Entries(&[
            ("runtimes/win-x64/native/onnxruntime.dll", "onnxruntime.dll"),
            (
                "runtimes/win-x64/native/onnxruntime_providers_shared.dll",
                "onnxruntime_providers_shared.dll",
            ),
        ]),
    },
    Download {
        url: "https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/1.15.4/microsoft.ai.directml.1.15.4.nupkg",
        bytes: 202_292_617,
        checksum: Checksum::Sha512(
            "/edn9WkEq8kP1T9l2HKckYq39uPF4ezdR5kI/AK0U1zysIYPerKsubcx1suAm3LD1dTQKFP7j16gIqR7xE7yhQ==",
        ),
        unpack: Unpack::Entries(&[("bin/x64-win/DirectML.dll", "DirectML.dll")]),
    },
];

/// The NuGet package of 1.24.4 ships `libonnxruntime.dylib` for Apple Silicon only.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const RUNTIME_DOWNLOADS: &[Download] = &[Download {
    url: "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.24.4/microsoft.ml.onnxruntime.1.24.4.nupkg",
    bytes: 125_194_303,
    checksum: Checksum::Sha512(
        "9d1BXfyvyzp0YfEKCPAzfqIsG6j4r4Exbaq/dJYHWt0YGuyw3jyr6+vbn12jr75QdICq80t24RiUCQiMzFwurA==",
    ),
    unpack: Unpack::Entries(&[(
        "runtimes/osx-arm64/native/libonnxruntime.dylib",
        "libonnxruntime.dylib",
    )]),
}];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const RUNTIME_DOWNLOADS: &[Download] = &[];

#[cfg(not(any(windows, target_os = "macos")))]
const RUNTIME_DOWNLOADS: &[Download] = &[];

#[cfg(windows)]
const PDFIUM_DOWNLOADS: &[Download] = &[Download {
    url: "https://api.nuget.org/v3-flatcontainer/bblanchon.pdfium.win32/158.0.8125/bblanchon.pdfium.win32.158.0.8125.nupkg",
    bytes: 11_023_573,
    checksum: Checksum::Sha512(
        "uMXm2JZPv4KQ8l82y84OGnq3nmjBs2TIHIHoxIEpuQgCw3DhgawrHZ64B4EVJgJTLJEyAq27oYrF4C8wAsssxw==",
    ),
    unpack: Unpack::Entries(&[("runtimes/win-x64/native/pdfium.dll", "pdfium.dll")]),
}];

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const PDFIUM_DOWNLOADS: &[Download] = &[Download {
    url: "https://api.nuget.org/v3-flatcontainer/bblanchon.pdfium.macos/158.0.8125/bblanchon.pdfium.macos.158.0.8125.nupkg",
    bytes: 7_132_903,
    checksum: Checksum::Sha512(
        "2Dkm2LqpQBG/R0c+mh9nfRg+ykolfvrnQkSsEjUhY4SZwIJEnveWxyVjfH0vAhOWhxQmwMLg83uq1G/fl1xSkw==",
    ),
    unpack: Unpack::Entries(&[("runtimes/osx-arm64/native/libpdfium.dylib", "libpdfium.dylib")]),
}];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const PDFIUM_DOWNLOADS: &[Download] = &[Download {
    url: "https://api.nuget.org/v3-flatcontainer/bblanchon.pdfium.macos/158.0.8125/bblanchon.pdfium.macos.158.0.8125.nupkg",
    bytes: 7_132_903,
    checksum: Checksum::Sha512(
        "2Dkm2LqpQBG/R0c+mh9nfRg+ykolfvrnQkSsEjUhY4SZwIJEnveWxyVjfH0vAhOWhxQmwMLg83uq1G/fl1xSkw==",
    ),
    unpack: Unpack::Entries(&[("runtimes/osx-x64/native/libpdfium.dylib", "libpdfium.dylib")]),
}];

#[cfg(not(any(windows, target_os = "macos")))]
const PDFIUM_DOWNLOADS: &[Download] = &[];

#[cfg(windows)]
const SDCPP_VULKAN_DOWNLOADS: &[Download] = &[Download {
    url: "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1122-3b12781/sd-master-3b12781-bin-win-vulkan-x64.zip",
    bytes: 30_956_351,
    checksum: Checksum::Sha256("efcc3785bcae2d1b2e70e1cee54fa163fa40e65185369f06ab8ff3e0c880b3aa"),
    unpack: Unpack::All,
}];
#[cfg(not(windows))]
const SDCPP_VULKAN_DOWNLOADS: &[Download] = &[];

#[cfg(windows)]
const SDCPP_CPU_DOWNLOADS: &[Download] = &[Download {
    url: "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1122-3b12781/sd-master-3b12781-bin-win-cpu-x64.zip",
    bytes: 19_005_519,
    checksum: Checksum::Sha256("8f77526a3e2768ed710ab0050c57dc4e1c6bac9ab163a63f899721e785def6aa"),
    unpack: Unpack::All,
}];
#[cfg(not(windows))]
const SDCPP_CPU_DOWNLOADS: &[Download] = &[];

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const SDCPP_METAL_DOWNLOADS: &[Download] = &[Download {
    url: "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1122-3b12781/sd-master-3b12781-bin-Darwin-macOS-26.7.2-arm64.zip",
    bytes: 39_644_163,
    checksum: Checksum::Sha256("efb501ec271393e43b08d25eed37ab26c154134fa495501923a5c17417b8782a"),
    unpack: Unpack::All,
}];
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
const SDCPP_METAL_DOWNLOADS: &[Download] = &[];

pub const ENTRIES: &[Entry] = &[
    Entry {
        id: RUNTIME,
        kind: Kind::Runtime,
        name: RUNTIME_NAME,
        license: RUNTIME_LICENSE,
        source: "https://onnxruntime.ai",
        downloads: RUNTIME_DOWNLOADS,
        requires: &[],
        spec: Spec::None,
    },
    Entry {
        id: PDFIUM,
        kind: Kind::Pdf,
        name: "PDFium 158.0.8125",
        license: "Apache-2.0",
        source: "https://github.com/bblanchon/pdfium-binaries",
        downloads: PDFIUM_DOWNLOADS,
        requires: &[],
        spec: Spec::None,
    },
    Entry {
        id: "sdcpp-vulkan",
        kind: Kind::Generator,
        name: "stable-diffusion.cpp (Vulkan)",
        license: "MIT",
        source: "https://github.com/leejet/stable-diffusion.cpp",
        downloads: SDCPP_VULKAN_DOWNLOADS,
        requires: &[],
        spec: Spec::None,
    },
    Entry {
        id: "sdcpp-cpu",
        kind: Kind::Generator,
        name: "stable-diffusion.cpp (CPU)",
        license: "MIT",
        source: "https://github.com/leejet/stable-diffusion.cpp",
        downloads: SDCPP_CPU_DOWNLOADS,
        requires: &[],
        spec: Spec::None,
    },
    Entry {
        id: "sdcpp-metal",
        kind: Kind::Generator,
        name: "stable-diffusion.cpp (Metal)",
        license: "MIT",
        source: "https://github.com/leejet/stable-diffusion.cpp",
        downloads: SDCPP_METAL_DOWNLOADS,
        requires: &[],
        spec: Spec::None,
    },
    Entry {
        id: "isnet-general-use",
        kind: Kind::Background,
        name: "IS-Net (general use)",
        license: "Apache-2.0",
        source: "https://github.com/xuebinqin/DIS",
        downloads: &[Download {
            url: "https://huggingface.co/tomjackson2023/rembg/resolve/main/isnet-general-use.onnx",
            bytes: 178_648_008,
            checksum: Checksum::Sha256(
                "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
            ),
            unpack: Unpack::File("isnet-general-use.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Matte {
            size: 1024,
            mean: IMAGENET_MEAN,
            std: UNIT_STD,
        },
    },
    Entry {
        id: "u2net",
        kind: Kind::Background,
        name: "U²-Net",
        license: "Apache-2.0",
        source: "https://github.com/xuebinqin/U-2-Net",
        downloads: &[Download {
            url: "https://huggingface.co/tomjackson2023/rembg/resolve/main/u2net.onnx",
            bytes: 175_997_641,
            checksum: Checksum::Sha256(
                "8d10d2f3bb75ae3b6d527c77944fc5e7dcd94b29809d47a739a7a728a912b491",
            ),
            unpack: Unpack::File("u2net.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Matte {
            size: 320,
            mean: IMAGENET_MEAN,
            std: IMAGENET_STD,
        },
    },
    Entry {
        id: "silueta",
        kind: Kind::Background,
        name: "Silueta",
        license: "Apache-2.0",
        source: "https://github.com/danielgatis/rembg",
        downloads: &[Download {
            url: "https://huggingface.co/tomjackson2023/rembg/resolve/main/silueta.onnx",
            bytes: 44_173_029,
            checksum: Checksum::Sha256(
                "75da6c8d2f8096ec743d071951be73b4a8bc7b3e51d9a6625d63644f90ffeedb",
            ),
            unpack: Unpack::File("silueta.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Matte {
            size: 320,
            mean: IMAGENET_MEAN,
            std: IMAGENET_STD,
        },
    },
    Entry {
        id: "u2netp",
        kind: Kind::Background,
        name: "U²-Net (small)",
        license: "Apache-2.0",
        source: "https://github.com/xuebinqin/U-2-Net",
        downloads: &[Download {
            url: "https://huggingface.co/tomjackson2023/rembg/resolve/main/u2netp.onnx",
            bytes: 4_574_861,
            checksum: Checksum::Sha256(
                "309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8",
            ),
            unpack: Unpack::File("u2netp.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Matte {
            size: 320,
            mean: IMAGENET_MEAN,
            std: IMAGENET_STD,
        },
    },
    Entry {
        id: "isnet-anime",
        kind: Kind::Background,
        name: "IS-Net (anime)",
        license: "Apache-2.0",
        source: "https://github.com/SkyTNT/anime-segmentation",
        downloads: &[Download {
            url: "https://huggingface.co/tomjackson2023/rembg/resolve/main/isnet-anime.onnx",
            bytes: 176_069_933,
            checksum: Checksum::Sha256(
                "f15622d853e8260172812b657053460e20806f04b9e05147d49af7bed31a6e99",
            ),
            unpack: Unpack::File("isnet-anime.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Matte {
            size: 1024,
            mean: IMAGENET_MEAN,
            std: UNIT_STD,
        },
    },
    Entry {
        id: "realesrgan-x4plus",
        kind: Kind::Upscale,
        name: "Real-ESRGAN x4plus",
        license: "BSD-3-Clause",
        source: "https://github.com/xinntao/Real-ESRGAN",
        downloads: &[Download {
            url: "https://huggingface.co/bukuroo/RealESRGAN-ONNX/resolve/main/real-esrgan-x4plus-128.onnx",
            bytes: 67_160_311,
            checksum: Checksum::Sha256(
                "6a6f4a3d58553d40fdd443d9e5f4b2deb9b52bef1ec2947700fc2167ac876c7d",
            ),
            unpack: Unpack::File("real-esrgan-x4plus-128.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Upscale,
    },
    Entry {
        id: "lama",
        kind: Kind::Inpaint,
        name: "LaMa",
        license: "Apache-2.0",
        source: "https://github.com/advimman/lama",
        downloads: &[Download {
            url: "https://huggingface.co/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx",
            bytes: 208_044_816,
            checksum: Checksum::Sha256(
                "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6",
            ),
            unpack: Unpack::File("lama_fp32.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Inpaint { size: 512 },
    },
    Entry {
        id: "gfpgan-1.4",
        kind: Kind::Faces,
        name: "GFPGAN 1.4",
        license: "Apache-2.0",
        source: "https://github.com/TencentARC/GFPGAN",
        downloads: &[Download {
            url: "https://huggingface.co/Neus/GFPGANv1.4/resolve/main/GFPGANv1.4.onnx",
            bytes: 340_256_686,
            checksum: Checksum::Sha256(
                "cd7311b8d9e13cdb1e208b12363182da58c7bf45e26d1aa67bbeac4751aae92e",
            ),
            unpack: Unpack::File("GFPGANv1.4.onnx"),
        }],
        requires: &[RUNTIME, DETECTOR],
        spec: Spec::Faces { size: 512 },
    },
    Entry {
        id: DETECTOR,
        kind: Kind::Detector,
        name: "YuNet face detector",
        license: "MIT",
        source: "https://github.com/opencv/opencv_zoo",
        downloads: &[Download {
            url: "https://huggingface.co/opencv/face_detection_yunet/resolve/main/face_detection_yunet_2023mar.onnx",
            bytes: 232_589,
            checksum: Checksum::Sha256(
                "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
            ),
            unpack: Unpack::File("face_detection_yunet_2023mar.onnx"),
        }],
        requires: NEEDS_RUNTIME,
        spec: Spec::Detector { size: 640 },
    },
];

pub fn find(id: &str) -> Option<&'static Entry> {
    ENTRIES
        .iter()
        .find(|entry| entry.id == id && entry.is_available())
}

pub fn of_kind(kind: Kind) -> impl Iterator<Item = &'static Entry> {
    ENTRIES
        .iter()
        .filter(move |entry| entry.kind == kind && entry.is_available())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    use super::*;

    #[test]
    fn ids_are_unique_and_requirements_exist() {
        let mut seen = HashSet::new();
        for entry in ENTRIES {
            assert!(seen.insert(entry.id), "duplicate id {}", entry.id);
            for required in entry.requires {
                assert!(
                    ENTRIES.iter().any(|other| other.id == *required),
                    "{} needs unknown {required}",
                    entry.id
                );
                assert_ne!(*required, entry.id);
            }
            if entry.is_offered() {
                assert!(!entry.downloads.is_empty(), "{} has no downloads", entry.id);
                for required in entry.requires {
                    assert!(
                        find(required).is_some()
                            || ENTRIES
                                .iter()
                                .any(|other| { other.id == *required && !other.is_offered() }),
                        "{} needs unknown {required}",
                        entry.id
                    );
                }
            }
        }
    }

    #[test]
    fn offered_downloads_are_pinned() {
        for entry in ENTRIES.iter().filter(|entry| entry.is_offered()) {
            for download in entry.downloads {
                assert!(download.url.starts_with("https://"), "{}", download.url);
                assert!(download.bytes > 0);
                match download.checksum {
                    Checksum::Sha256(hex) => {
                        assert_eq!(hex.len(), 64, "{}", entry.id);
                        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
                    }
                    Checksum::Sha512(encoded) => {
                        assert_eq!(STANDARD.decode(encoded).unwrap().len(), 64, "{}", entry.id);
                    }
                }
            }
        }
    }

    #[test]
    fn every_offered_entry_has_a_main_file() {
        for entry in ENTRIES.iter().filter(|entry| entry.is_offered()) {
            assert!(!entry.main_file().is_empty(), "{}", entry.id);
        }
    }

    #[test]
    fn models_need_the_runtime() {
        for entry in ENTRIES.iter().filter(|entry| !entry.kind.is_component()) {
            assert!(entry.requires.contains(&RUNTIME), "{}", entry.id);
        }
    }
}
