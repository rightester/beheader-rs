pub mod image;
pub mod mp4;
pub mod polyglot;
pub mod utils;

pub use image::convert_image_to_png;
pub use polyglot::{append_zip_to_output, build_polyglot, PolyglotConfig, PolyglotResult};

#[cfg(target_arch = "wasm32")]
mod wasm_api;

#[cfg(target_arch = "wasm32")]
pub use wasm_api::*;
