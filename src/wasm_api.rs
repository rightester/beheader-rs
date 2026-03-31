use wasm_bindgen::prelude::*;

use crate::polyglot::{build_polyglot, PolyglotConfig};

#[wasm_bindgen]
pub fn wasm_build_polyglot(
    png_data: Vec<u8>,
    mp4_data: Vec<u8>,
    html_content: Option<String>,
    pdf_data: Option<Vec<u8>>,
    extra_data: Option<Vec<u8>>,
) -> Result<Vec<u8>, JsError> {
    let config = PolyglotConfig {
        png_data,
        mp4_data,
        html_content,
        pdf_data,
        extra_data,
    };

    let result = build_polyglot(&config).map_err(|e| JsError::new(&e.to_string()))?;

    let mut output = result.data;
    if let Some(suffix) = result.pdf_suffix {
        output.extend(suffix);
    }

    Ok(output)
}
