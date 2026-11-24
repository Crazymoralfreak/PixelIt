//! Image operations. Each module implements the steps of the pipeline model
//! in [`crate::pipeline`] and can also be called directly.

pub mod adjust;
pub mod annotate;
pub mod draw;
pub mod filter;
pub mod geometry;
pub mod resize;
pub mod watermark;
