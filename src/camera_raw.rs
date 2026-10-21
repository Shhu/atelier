//! Camera Raw: the photographic development of Filter › Camera Raw Filter
//! and of opened camera RAW files. Images are developed in linear light, in
//! floating point: white balance and exposure, local tone (highlights and
//! shadows recovered on an edge-preserving base layer, like the real
//! thing), tone curves, color, detail and effects, then 8 bits.
//!
//! RAW files: DNG is decoded (demosaicked, colored by its matrices); the
//! other formats open through the full-size preview they carry (see
//! [`raw`]).

mod planes;
pub mod raw;
mod render;
mod settings;
mod source;
#[cfg(test)]
mod tests;
mod white;

pub use render::{Analysis, Frame, Visual, apply, parametric_samples, render};
pub use settings::{
    BUILT_IN_PRESETS, BuiltIn, Preset, Profile, Settings, VignetteStyle, Wheel, WhiteBalance,
    load_presets, save_presets,
};
pub use source::Source;
pub use white::{TEMPERATURE_RANGE, TINT_RANGE};
