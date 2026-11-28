//! What this computer can run: video cards, memory, processor threads.

use serde::Serialize;

#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
#[cfg(windows)]
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use crate::config::{AiSettings, DeviceChoice};

/// Video memory from which generation on the video card is comfortable.
const GENERATION_VRAM_MB: u64 = 6 * 1024;

/// Video memory below which local models run better on the processor.
const MODEL_VRAM_MB: u64 = 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    /// Index DirectML uses for this card.
    pub index: u32,
    pub name: String,
    pub vendor: u32,
    /// Dedicated video memory.
    pub memory_mb: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    pub gpus: Vec<Gpu>,
    pub ram_mb: u64,
    pub threads: usize,
}

/// What the setup wizard proposes for this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Advice {
    /// Whether local models should use the video card.
    pub use_gpu: bool,
    /// Index of the proposed card; `-1` without one.
    pub gpu: i32,
    /// Catalogue id of the proposed build of stable-diffusion.cpp.
    pub generator: &'static str,
    /// Whether generation on this computer is expected to be slow.
    pub slow_generation: bool,
}

#[cfg(windows)]
fn gpus() -> Vec<Gpu> {
    let mut found = Vec::new();
    // SAFETY: plain COM calls; every returned interface is owned by a wrapper
    // that releases it, and the description is a plain structure.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return found;
        };
        for index in 0..16u32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let Ok(description) = adapter.GetDesc1() else {
                continue;
            };
            if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let length = description
                .Description
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(description.Description.len());
            found.push(Gpu {
                index,
                name: String::from_utf16_lossy(&description.Description[..length]),
                vendor: description.VendorId,
                memory_mb: (description.DedicatedVideoMemory as u64) / (1024 * 1024),
            });
        }
    }
    found
}

#[cfg(windows)]
fn ram_mb() -> u64 {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..MEMORYSTATUSEX::default()
    };
    // SAFETY: the structure is initialised with its own size, as the call requires.
    match unsafe { GlobalMemoryStatusEx(&raw mut status) } {
        Ok(()) => status.ullTotalPhys / (1024 * 1024),
        Err(_) => 0,
    }
}

/// Memory and the shared GPU of Apple Silicon.
#[cfg(target_os = "macos")]
fn ram_mb() -> u64 {
    use std::process::Command;
    let Ok(output) = Command::new("sysctl").args(["-n", "hw.memsize"]).output() else {
        return 0;
    };
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn gpus() -> Vec<Gpu> {
    // Apple Silicon: one GPU that shares the system memory. Intel Macs have no
    // catalogue entry for a local generator that uses a discrete card here.
    if cfg!(target_arch = "aarch64") {
        let memory_mb = ram_mb();
        vec![Gpu {
            index: 0,
            name: String::from("Apple Silicon"),
            vendor: 0,
            memory_mb,
        }]
    } else {
        Vec::new()
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn gpus() -> Vec<Gpu> {
    Vec::new()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn ram_mb() -> u64 {
    0
}

pub fn detect() -> Hardware {
    Hardware {
        gpus: gpus(),
        ram_mb: ram_mb(),
        threads: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
    }
}

impl Hardware {
    /// The card with the most dedicated memory.
    pub fn best_gpu(&self) -> Option<&Gpu> {
        self.gpus.iter().max_by_key(|gpu| gpu.memory_mb)
    }

    pub fn advice(&self) -> Advice {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return Advice {
                use_gpu: true,
                gpu: 0,
                generator: "sdcpp-metal",
                slow_generation: self.ram_mb < GENERATION_VRAM_MB,
            };
        }
        match self.best_gpu() {
            Some(gpu) if gpu.memory_mb >= MODEL_VRAM_MB => Advice {
                use_gpu: true,
                gpu: gpu.index as i32,
                generator: "sdcpp-vulkan",
                slow_generation: gpu.memory_mb < GENERATION_VRAM_MB,
            },
            Some(_) | None => Advice {
                use_gpu: false,
                gpu: -1,
                generator: "sdcpp-cpu",
                slow_generation: true,
            },
        }
    }

    /// Device and card a model session should use: `(use the card, its index)`.
    pub fn device(&self, settings: &AiSettings) -> (bool, i32) {
        let advice = self.advice();
        let chosen = if settings.gpu >= 0 {
            settings.gpu
        } else {
            advice.gpu.max(0)
        };
        match settings.device {
            DeviceChoice::Cpu => (false, 0),
            DeviceChoice::Gpu => (true, chosen),
            DeviceChoice::Auto => (advice.use_gpu, chosen),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(index: u32, memory_mb: u64) -> Gpu {
        Gpu {
            index,
            name: format!("Card {index}"),
            vendor: 0x10de,
            memory_mb,
        }
    }

    #[test]
    fn detection_does_not_fail() {
        let hardware = detect();
        assert!(hardware.threads >= 1);
    }

    #[test]
    fn advice_follows_video_memory() {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            let advice = Hardware {
                ram_mb: 16 * 1024,
                ..Hardware::default()
            }
            .advice();
            assert_eq!(advice.generator, "sdcpp-metal");
            assert!(advice.use_gpu);
            return;
        }

        let none = Hardware::default();
        assert_eq!(none.advice().generator, "sdcpp-cpu");
        assert!(!none.advice().use_gpu);

        let integrated = Hardware {
            gpus: vec![card(0, 128)],
            ..Hardware::default()
        };
        assert!(!integrated.advice().use_gpu);

        let strong = Hardware {
            gpus: vec![card(0, 512), card(1, 12 * 1024)],
            ..Hardware::default()
        };
        let advice = strong.advice();
        assert!(advice.use_gpu);
        assert_eq!(advice.gpu, 1);
        assert_eq!(advice.generator, "sdcpp-vulkan");
        assert!(!advice.slow_generation);
    }

    #[test]
    fn settings_override_the_advice() {
        let hardware = Hardware {
            gpus: vec![card(0, 8 * 1024), card(1, 4 * 1024)],
            ..Hardware::default()
        };
        let mut settings = AiSettings::default();
        assert_eq!(hardware.device(&settings), (true, 0));
        settings.gpu = 1;
        assert_eq!(hardware.device(&settings), (true, 1));
        settings.device = DeviceChoice::Cpu;
        assert_eq!(hardware.device(&settings), (false, 0));
        settings.device = DeviceChoice::Gpu;
        assert_eq!(Hardware::default().device(&settings), (true, 1));
    }
}
