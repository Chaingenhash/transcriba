use super::{subtitle, DocumentMeta, RenderError};
use crate::reflow::{format_timestamp, Block};
use docx_rs::*;

pub fn render_docx(blocks: &[Block], meta: &DocumentMeta) -> Result<Vec<u8>, RenderError> {
    let mut doc = Docx::new()
        .add_paragraph(Paragraph::new().add_run(Run::new().add_text(&meta.title).bold().size(36)))
        .add_paragraph(Paragraph::new().add_run(Run::new().add_text(subtitle(meta)).size(18)));

    for block in blocks {
        doc = match block {
            Block::Heading { from, to } => doc.add_paragraph(
                Paragraph::new().add_run(
                    Run::new()
                        .add_text(format!(
                            "{} – {}",
                            format_timestamp(*from),
                            format_timestamp(*to)
                        ))
                        .bold()
                        .size(24),
                ),
            ),
            Block::Para { start, text } => doc.add_paragraph(
                Paragraph::new()
                    .add_run(
                        Run::new()
                            .add_text(format!("[{}] ", format_timestamp(*start)))
                            .size(16),
                    )
                    .add_run(Run::new().add_text(text).size(22)),
            ),
        };
    }

    let mut buf = Vec::new();
    doc.build()
        .pack(std::io::Cursor::new(&mut buf))
        .map_err(|e| RenderError::Docx(e.to_string()))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflow::Block;
    use std::io::Read;

    fn meta() -> DocumentMeta {
        DocumentMeta {
            title: "Reuniao de teste".to_string(),
            duration: 3510.88,
            backend: "CPU (12 threads)".to_string(),
        }
    }

    /// Unzips the .docx and returns the raw document.xml. Byte-level proof the
    /// text survived, rather than trusting the builder silently produced content.
    fn document_xml(bytes: &[u8]) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
        let mut file = archive
            .by_name("word/document.xml")
            .expect("has document.xml");
        let mut xml = String::new();
        file.read_to_string(&mut xml).expect("readable xml");
        xml
    }

    #[test]
    fn writes_paragraph_text_into_the_document() {
        let blocks = vec![Block::Para {
            start: 5.0,
            text: "Muito boa tarde a todos.".into(),
        }];
        let bytes = render_docx(&blocks, &meta()).expect("renders");
        let xml = document_xml(&bytes);
        assert!(xml.contains("Muito boa tarde a todos."));
    }

    #[test]
    fn includes_a_timestamp_marker_per_paragraph() {
        let blocks = vec![Block::Para {
            start: 65.0,
            text: "Texto.".into(),
        }];
        let xml = document_xml(&render_docx(&blocks, &meta()).unwrap());
        assert!(xml.contains("[1:05]"));
    }

    #[test]
    fn includes_headings_and_title_and_backend() {
        let blocks = vec![
            Block::Heading {
                from: 0.0,
                to: 600.0,
            },
            Block::Para {
                start: 1.0,
                text: "Texto.".into(),
            },
        ];
        let xml = document_xml(&render_docx(&blocks, &meta()).unwrap());
        assert!(xml.contains("0:00"));
        assert!(xml.contains("Reuniao de teste"));
        assert!(xml.contains("CPU (12 threads)"));
    }

    #[test]
    fn preserves_portuguese_diacritics() {
        let blocks = vec![Block::Para {
            start: 0.0,
            text: "Assembleia de Freguesia, aprovação.".into(),
        }];
        let xml = document_xml(&render_docx(&blocks, &meta()).unwrap());
        assert!(xml.contains("aprovação"));
    }
}
