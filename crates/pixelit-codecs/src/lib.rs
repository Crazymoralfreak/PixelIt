//! PixelIt codecs: reading and writing image files.
//!
//! Every decoder returns pixels in the core layout: interleaved RGBA, straight
//! alpha, sRGB, with the EXIF orientation already applied. Every encoder takes
//! pixels in that layout. Colour profiles, EXIF and XMP travel separately in
//! [`Metadata`].

#![deny(unsafe_code)]

pub mod color;
pub mod decode;
pub mod encode;
pub mod info;
pub mod metadata;

pub use decode::{decode_bytes, decode_file, probe_bytes, probe_file};
pub use encode::{encode, encode_animation, encode_media, EncodeOptions};
pub use info::{DecodeOptions, Decoded, SourceInfo};
pub use metadata::Metadata;
