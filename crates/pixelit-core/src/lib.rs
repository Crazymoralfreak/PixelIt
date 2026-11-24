//! PixelIt core: pixel buffers, image operations and the pipeline model.
//!
//! The crate knows nothing about files, codecs, the network or the interface.
//! It takes decoded pixels, changes them and hands them back, which keeps it
//! small, fast to test and free of `unsafe`.

#![forbid(unsafe_code)]

pub mod cancel;
pub mod color;
pub mod error;
pub mod format;
pub mod hash;
pub mod image;
pub mod limits;
pub mod naming;
pub mod ops;
pub mod pipeline;
mod rows;
pub mod text;

pub use cancel::CancelToken;
pub use color::Color;
pub use error::{Error, Result};
pub use format::ImageFormat;
pub use image::{Animation, Depth, Frame, Image, Media, Pixels};
pub use limits::Limits;
pub use pipeline::{ApplyContext, Op, OutputSpec, Pipeline, Step};
