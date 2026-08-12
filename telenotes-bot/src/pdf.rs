//! Text extraction for PDFs, so document uploads can be chunked and embedded
//! the same way notes are. Best-effort: a PDF that's scanned images with no
//! text layer, or one `pdf-extract` chokes on, just yields no chunks --
//! the file itself is still saved fine, it's only semantic search that
//! misses it.

use anyhow::Result;

pub fn extract_text(bytes: &[u8]) -> Result<String> {
    let text = pdf_extract::extract_text_from_mem(bytes)?;
    Ok(text)
}
