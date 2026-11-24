//! Screen capture: one picture of the whole desktop, all monitors included.
//! The editor opens it, and the crop tool cuts out the part that is needed.

use std::fs;
use std::mem::size_of;
use std::path::PathBuf;

use chrono::Local;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ, ROP_CODE, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use pixelit_codecs::{encode, EncodeOptions};
use pixelit_core::{CancelToken, Error, Image, ImageFormat, Result};
use pixelit_engine::Paths;

/// The device contexts and the bitmap of one capture, released together.
struct Surface {
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the handles were created by `screen()` and are released
        // once, in the order GDI asks for: the bitmap leaves the context
        // before it is deleted.
        unsafe {
            if !self.memory.is_invalid() {
                if !self.previous.is_invalid() {
                    SelectObject(self.memory, self.previous);
                }
                let _ = DeleteDC(self.memory);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            ReleaseDC(None, self.screen);
        }
    }
}

fn failed() -> Error {
    Error::new("captureFailed")
}

/// Rows of a 32-bit capture as opaque RGBA: GDI gives blue first and leaves
/// the fourth byte undefined.
fn to_rgba(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    pixels
}

/// Copies the virtual screen into an image.
pub fn screen() -> Result<Image> {
    // SAFETY: plain GDI calls on handles checked right after they are
    // created; `Surface` releases them on every path. The pixel buffer is as
    // large as the bitmap header describes.
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return Err(failed());
        }
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(failed());
        }
        let mut surface = Surface {
            screen,
            memory: HDC::default(),
            bitmap: HBITMAP::default(),
            previous: HGDIOBJ::default(),
        };
        surface.memory = CreateCompatibleDC(Some(screen));
        surface.bitmap = CreateCompatibleBitmap(screen, width, height);
        if surface.memory.is_invalid() || surface.bitmap.is_invalid() {
            return Err(failed());
        }
        surface.previous = SelectObject(surface.memory, surface.bitmap.into());
        // Layered windows (menus, tooltips) are part of what the user sees.
        let copy = ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0);
        BitBlt(
            surface.memory,
            0,
            0,
            width,
            height,
            Some(screen),
            left,
            top,
            copy,
        )
        .map_err(|_| failed())?;
        // The bitmap must be out of the context while its bits are read.
        SelectObject(surface.memory, surface.previous);
        surface.previous = HGDIOBJ::default();

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // A negative height asks for rows from the top down.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let rows = GetDIBits(
            surface.memory,
            surface.bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &raw mut info,
            DIB_RGB_COLORS,
        );
        if rows != height {
            return Err(failed());
        }
        let mut image = Image::from_rgba8(width as u32, height as u32, to_rgba(pixels))?;
        image.set_has_alpha(false);
        Ok(image)
    }
}

/// Writes a capture into the data folder as PNG and returns its path.
pub fn save(paths: &Paths, image: &Image) -> Result<PathBuf> {
    let folder = paths.root.join("captures");
    fs::create_dir_all(&folder)?;
    let bytes = encode(
        image,
        &EncodeOptions::new(ImageFormat::Png),
        &CancelToken::new(),
    )?;
    let stamp = Local::now().format("%Y-%m-%d %H-%M-%S");
    let mut path = folder.join(format!("PixelIt {stamp}.png"));
    let mut number = 2;
    while path.exists() {
        path = folder.join(format!("PixelIt {stamp} ({number}).png"));
        number += 1;
    }
    fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_rows_become_opaque_rgba() {
        let rgba = to_rgba(vec![10, 20, 30, 0, 1, 2, 3, 77]);
        assert_eq!(rgba, [30, 20, 10, 255, 3, 2, 1, 255]);
    }

    #[test]
    fn captures_are_numbered() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let image = Image::from_rgba8(1, 1, vec![1, 2, 3, 255]).unwrap();
        let first = save(&paths, &image).unwrap();
        let second = save(&paths, &image).unwrap();
        assert!(first.is_file() && second.is_file());
        assert_ne!(first, second);
    }
}
