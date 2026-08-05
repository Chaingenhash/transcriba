pub mod docx;

/// Metadata rendered into the document header.
#[derive(Debug, Clone)]
pub struct DocumentMeta {
    pub title: String,
    pub duration: f64,
    pub backend: String,
}

#[derive(Debug)]
pub enum RenderError {
    Docx(String),
    Pdf(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Docx(m) => write!(f, "could not build the Word document: {m}"),
            RenderError::Pdf(m) => write!(f, "could not build the PDF: {m}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Shared disclaimer. Machine output is never presented as verified.
pub fn subtitle(meta: &DocumentMeta) -> String {
    format!(
        "Duração {} · transcrição automática ({}) · texto não revisto — \
verificar nomes, valores e votações contra o áudio original.",
        crate::reflow::format_timestamp(meta.duration),
        meta.backend,
    )
}
