//! Size limits that protect against decompression bombs and runaway memory use.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Limits {
    /// Largest accepted width or height, in pixels.
    pub max_side: u32,
    /// Largest accepted pixel count of one image or frame.
    pub max_pixels: u64,
    /// Largest accepted input file, in bytes.
    pub max_file_bytes: u64,
    /// Largest accepted total pixel count of an animation.
    pub max_animation_pixels: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_side: 65_535,
            max_pixels: 400_000_000,
            max_file_bytes: 2 * 1024 * 1024 * 1024,
            max_animation_pixels: 2_000_000_000,
        }
    }
}

impl Limits {
    /// Checks the dimensions of one image before any allocation happens.
    pub fn check_size(&self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(Error::new("emptyImage"));
        }
        let pixels = u64::from(width) * u64::from(height);
        if width > self.max_side || height > self.max_side || pixels > self.max_pixels {
            return Err(Error::new("tooLarge")
                .with("width", width)
                .with("height", height));
        }
        Ok(())
    }

    /// Checks the size of an input file.
    pub fn check_file(&self, bytes: u64) -> Result<()> {
        if bytes > self.max_file_bytes {
            return Err(Error::new("fileTooLarge").with("bytes", bytes));
        }
        Ok(())
    }

    /// Checks the total size of an animation.
    pub fn check_animation(&self, width: u32, height: u32, frames: u64) -> Result<()> {
        self.check_size(width, height)?;
        let pixels = u64::from(width) * u64::from(height);
        if pixels.saturating_mul(frames) > self.max_animation_pixels {
            return Err(Error::new("tooLarge")
                .with("width", width)
                .with("height", height));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_accept_common_sizes() {
        let limits = Limits::default();
        assert!(limits.check_size(8000, 6000).is_ok());
        assert!(limits.check_file(50 * 1024 * 1024).is_ok());
        assert!(limits.check_animation(1920, 1080, 300).is_ok());
    }

    #[test]
    fn rejects_bombs() {
        let limits = Limits::default();
        assert_eq!(limits.check_size(0, 10).unwrap_err().code, "emptyImage");
        assert_eq!(limits.check_size(70_000, 10).unwrap_err().code, "tooLarge");
        assert_eq!(
            limits.check_size(60_000, 60_000).unwrap_err().code,
            "tooLarge"
        );
        assert_eq!(
            limits.check_file(u64::MAX).unwrap_err().code,
            "fileTooLarge"
        );
        assert_eq!(
            limits.check_animation(4000, 4000, 1000).unwrap_err().code,
            "tooLarge"
        );
    }
}
