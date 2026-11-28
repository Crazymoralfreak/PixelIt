//! HEIC/HEIF decoding.
//!
//! On Windows the Imaging Component decodes HEIC when the HEIF Image Extensions
//! and the HEVC Video Extensions are installed; without them the decoder
//! reports a missing codec. On macOS the system `sips` tool turns the file
//! into a TIFF that the ordinary decoder then reads.

use pixelit_core::Result;

use super::Loaded;
use crate::info::DecodeOptions;

#[cfg(windows)]
mod wic {
    use std::ptr;

    use pixelit_core::{Error, Image, Media, Result};
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat24bppRGB,
        GUID_WICPixelFormat32bppBGR, GUID_WICPixelFormat32bppBGRA, GUID_WICPixelFormat32bppRGBA,
        GUID_WICPixelFormat64bppRGBA, GUID_WICPixelFormat8bppGray, IWICBitmapFrameDecode,
        IWICImagingFactory, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Shell::SHCreateMemStream;

    use super::super::{bad, bmff_size, Loaded};
    use crate::info::DecodeOptions;
    use crate::metadata::Metadata;

    /// Balances a successful COM initialisation of the current thread.
    struct ComGuard(bool);

    impl ComGuard {
        fn new() -> Self {
            // SAFETY: plain COM initialisation; a failure (for example a thread
            // already initialised in another mode) leaves COM usable and is
            // not balanced by `CoUninitialize`.
            let initialised = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            Self(initialised)
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: pairs with the successful `CoInitializeEx` above.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// Reads the ICC profile attached to the frame.
    fn profile(factory: &IWICImagingFactory, frame: &IWICBitmapFrameDecode) -> Option<Vec<u8>> {
        // SAFETY: every pointer passed to WIC refers to a live local value and
        // every buffer is passed with its real length.
        unsafe {
            let context = factory.CreateColorContext().ok()?;
            let mut contexts = [Some(context)];
            let mut count = 0_u32;
            frame.GetColorContexts(&mut contexts, &raw mut count).ok()?;
            if count == 0 {
                return None;
            }
            let context = contexts[0].take()?;
            let mut size = 0_u32;
            // The first call only reports the size; its status is irrelevant.
            let _ = context.GetProfileBytes(&mut [], &raw mut size);
            if size == 0 {
                return None;
            }
            let mut data = vec![0_u8; size as usize];
            context.GetProfileBytes(&mut data, &raw mut size).ok()?;
            data.truncate(size as usize);
            Some(data)
        }
    }

    pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
        if let Some((width, height)) = bmff_size(bytes) {
            options.limits.check_size(width, height)?;
        }
        let _com = ComGuard::new();
        // SAFETY: the COM objects are used on the thread that created them and
        // released before `_com` is dropped; `bytes` outlives the stream that
        // copies it; pixel buffers are passed with their real length.
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(bad)?;
            let stream = SHCreateMemStream(Some(bytes)).ok_or_else(|| Error::new("badFile"))?;
            let decoder = factory
                .CreateDecoderFromStream(&stream, ptr::null(), WICDecodeMetadataCacheOnDemand)
                .map_err(|error| Error::new("heicCodecMissing").detail(error))?;
            let frame = decoder.GetFrame(0).map_err(bad)?;

            let (mut width, mut height) = (0_u32, 0_u32);
            frame
                .GetSize(&raw mut width, &raw mut height)
                .map_err(bad)?;
            options.limits.check_size(width, height)?;
            let icc = profile(&factory, &frame);
            let native = frame.GetPixelFormat().map_err(bad)?;
            let eight_bit = [
                GUID_WICPixelFormat24bppBGR,
                GUID_WICPixelFormat24bppRGB,
                GUID_WICPixelFormat32bppBGR,
                GUID_WICPixelFormat32bppBGRA,
                GUID_WICPixelFormat32bppRGBA,
                GUID_WICPixelFormat8bppGray,
            ]
            .contains(&native);
            let deep = options.deep && !eight_bit;

            let image = if deep {
                let converted =
                    WICConvertBitmapSource(&GUID_WICPixelFormat64bppRGBA, &frame).map_err(bad)?;
                let mut data = vec![0_u16; width as usize * height as usize * 4];
                converted
                    .CopyPixels(
                        ptr::null(),
                        width * 8,
                        bytemuck::cast_slice_mut::<u16, u8>(&mut data),
                    )
                    .map_err(bad)?;
                Image::from_rgba16(width, height, data)?
            } else {
                let converted =
                    WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &frame).map_err(bad)?;
                let mut data = vec![0_u8; width as usize * height as usize * 4];
                converted
                    .CopyPixels(ptr::null(), width * 4, &mut data)
                    .map_err(bad)?;
                Image::from_rgba8(width, height, data)?
            };
            Ok(Loaded {
                media: Media::Still(image),
                bits: if eight_bit { 8 } else { 10 },
                metadata: Metadata {
                    exif: None,
                    icc,
                    xmp: None,
                },
                // The Windows HEIF decoder applies the stored rotation.
                orientation: 1,
                pages: 0,
            })
        }
    }
}

#[cfg(windows)]
pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    wic::decode(bytes, options)
}

/// Turns HEIC into TIFF with the system tool, then reads the TIFF.
#[cfg(target_os = "macos")]
mod sips {
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use pixelit_core::{Error, ImageFormat, Result};

    use super::super::{generic, Loaded};
    use crate::info::DecodeOptions;

    pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let folder = std::env::temp_dir().join(format!("pixelit-heic-{stamp}"));
        fs::create_dir_all(&folder)?;
        let input = folder.join("in.heic");
        let output = folder.join("out.tiff");
        let result = (|| -> Result<Loaded> {
            fs::write(&input, bytes)?;
            let status = Command::new("sips")
                .args([
                    "-s",
                    "format",
                    "tiff",
                    &input.display().to_string(),
                    "--out",
                    &output.display().to_string(),
                ])
                .status()
                .map_err(|error| Error::new("heicCodecMissingMac").detail(error))?;
            if !status.success() || !output.is_file() {
                return Err(Error::new("heicCodecMissingMac"));
            }
            let tiff = fs::read(&output)?;
            generic::decode(&tiff, ImageFormat::Tiff, options)
        })();
        let _ = fs::remove_dir_all(&folder);
        result
    }
}

#[cfg(target_os = "macos")]
pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    sips::decode(bytes, options)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn decode(_bytes: &[u8], _options: &DecodeOptions) -> Result<Loaded> {
    Err(pixelit_core::Error::new("unsupportedFormat").detail("HEIC"))
}
