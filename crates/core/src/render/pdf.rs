use super::{subtitle, DocumentMeta, RenderError};
use crate::reflow::{format_timestamp, Block};
use genpdf::{elements, fonts, style, Document, Element, SimplePageDecorator};
use std::path::Path;

pub fn render_pdf(
    blocks: &[Block],
    meta: &DocumentMeta,
    font_dir: &Path,
) -> Result<Vec<u8>, RenderError> {
    let family = fonts::from_files(font_dir, "LiberationSerif", None)
        .map_err(|e| RenderError::Pdf(format!("could not load fonts from {font_dir:?}: {e}")))?;

    let mut doc = Document::new(family);
    doc.set_title(&meta.title);
    doc.set_minimal_conformance();

    let mut decorator = SimplePageDecorator::new();
    decorator.set_margins(20);
    doc.set_page_decorator(decorator);

    doc.push(
        elements::Paragraph::new(&meta.title).styled(style::Style::new().bold().with_font_size(18)),
    );
    doc.push(
        elements::Paragraph::new(subtitle(meta)).styled(style::Style::new().with_font_size(8)),
    );
    doc.push(elements::Break::new(1));

    for block in blocks {
        match block {
            Block::Heading { from, to } => {
                doc.push(elements::Break::new(1));
                doc.push(
                    elements::Paragraph::new(format!(
                        "{} – {}",
                        format_timestamp(*from),
                        format_timestamp(*to)
                    ))
                    .styled(style::Style::new().bold().with_font_size(12)),
                );
            }
            Block::Para { start, text } => {
                doc.push(
                    elements::Paragraph::new(format!("[{}] {}", format_timestamp(*start), text))
                        .styled(style::Style::new().with_font_size(11)),
                );
            }
        }
    }

    let mut buf = Vec::new();
    doc.render(&mut buf)
        .map_err(|e| RenderError::Pdf(e.to_string()))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflow::Block;

    fn meta() -> DocumentMeta {
        DocumentMeta {
            title: "Reuniao de teste".to_string(),
            duration: 3510.88,
            backend: "CPU (12 threads)".to_string(),
        }
    }

    fn font_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts")
    }

    #[test]
    fn produces_a_valid_pdf_header() {
        let blocks = vec![Block::Para {
            start: 0.0,
            text: "Texto.".into(),
        }];
        let bytes = render_pdf(&blocks, &meta(), &font_dir()).expect("renders");
        assert!(bytes.starts_with(b"%PDF-"), "output must be a PDF");
    }

    #[test]
    fn paragraph_text_is_extractable() {
        let blocks = vec![Block::Para {
            start: 0.0,
            text: "Muito boa tarde a todos.".into(),
        }];
        let bytes = render_pdf(&blocks, &meta(), &font_dir()).unwrap();
        let text = pdf_extract::extract_text_from_mem(&bytes).expect("extractable");
        assert!(text.contains("Muito boa tarde"));
    }

    #[test]
    fn long_transcripts_keep_all_content() {
        // 300 paragraphs cannot fit one page, so this also proves pagination did
        // not silently drop the overflow — the failure mode that matters.
        let blocks: Vec<_> = (0..300)
            .map(|i| Block::Para {
                start: i as f64,
                text: format!("Paragrafo numero {i} deste teste."),
            })
            .collect();
        let bytes = render_pdf(&blocks, &meta(), &font_dir()).unwrap();
        let text = pdf_extract::extract_text_from_mem(&bytes).expect("extractable");
        assert!(
            text.contains("Paragrafo numero 0 "),
            "first paragraph missing"
        );
        assert!(
            text.contains("Paragrafo numero 299 "),
            "last paragraph missing"
        );
    }
}
