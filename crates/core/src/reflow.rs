//! Turns whisper's short subtitle cues into readable paragraphs.
//!
//! Ported from a Python implementation validated on a 58m30s Portuguese
//! recording: 2553 cues collapsed to 141 sentences and 58 paragraphs.

const PAUSE_BREAK_SECS: f64 = 1.0;
const MAX_PARA_CHARS: usize = 700;
const SECTION_SECS: f64 = 600.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading { from: f64, to: f64 },
    Para { start: f64, text: String },
}

struct Sentence {
    start: f64,
    end: f64,
    text: String,
}

/// True when `text` ends a sentence, allowing one trailing closing quote or bracket.
fn closes_sentence(text: &str) -> bool {
    let trimmed = text.trim_end_matches(['\'', '"', ')', ']']);
    matches!(
        trimmed.chars().last(),
        Some('.') | Some('!') | Some('?') | Some('…')
    )
}

fn to_sentences(cues: &[Cue]) -> Vec<Sentence> {
    let mut out = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    let mut start = None;
    let mut end = 0.0;

    for cue in cues {
        if start.is_none() {
            start = Some(cue.start);
        }
        end = cue.end;
        let trimmed = cue.text.trim();
        buf.push(trimmed);
        if closes_sentence(trimmed) {
            out.push(Sentence {
                start: start.take().unwrap(),
                end,
                text: buf.join(" "),
            });
            buf.clear();
        }
    }
    // Trailing cues with no terminator must not be dropped.
    if let Some(s) = start {
        out.push(Sentence {
            start: s,
            end,
            text: buf.join(" "),
        });
    }
    out
}

pub fn reflow(cues: &[Cue]) -> Vec<Block> {
    let sentences = to_sentences(cues);
    let mut blocks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut para_start = 0.0;
    let mut prev_end: Option<f64> = None;
    let mut section: i64 = -1;

    // Captures nothing, so it needs no `mut` — `-D warnings` in CI rejects an unused mut.
    let flush = |blocks: &mut Vec<Block>, current: &mut Vec<String>, start: f64| {
        if !current.is_empty() {
            blocks.push(Block::Para {
                start,
                text: current.join(" "),
            });
            current.clear();
        }
    };

    for s in &sentences {
        let gap = prev_end.map_or(0.0, |p| s.start - p);
        let too_long = current.iter().map(|t| t.len()).sum::<usize>() > MAX_PARA_CHARS;

        if !current.is_empty() && (gap > PAUSE_BREAK_SECS || too_long) {
            flush(&mut blocks, &mut current, para_start);
        }
        if current.is_empty() {
            para_start = s.start;
            let block = (s.start / SECTION_SECS) as i64;
            if block != section {
                section = block;
                let from = block as f64 * SECTION_SECS;
                blocks.push(Block::Heading {
                    from,
                    to: from + SECTION_SECS,
                });
            }
        }
        current.push(s.text.clone());
        prev_end = Some(s.end);
    }
    flush(&mut blocks, &mut current, para_start);
    blocks
}

pub fn format_timestamp(seconds: f64) -> String {
    let total = seconds as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: f64, end: f64, text: &str) -> Cue {
        Cue {
            start,
            end,
            text: text.to_string(),
        }
    }

    #[test]
    fn merges_cues_into_one_sentence_until_terminal_punctuation() {
        let cues = vec![cue(0.0, 1.0, "Muito boa tarde"), cue(1.0, 2.0, "a todos.")];
        let blocks = reflow(&cues);
        let paras: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(paras, vec!["Muito boa tarde a todos."]);
    }

    #[test]
    fn breaks_paragraph_on_pause_longer_than_one_second() {
        let cues = vec![
            cue(0.0, 1.0, "Primeira frase."),
            cue(3.0, 4.0, "Segunda frase."),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn keeps_sentences_together_when_pause_is_short() {
        let cues = vec![
            cue(0.0, 1.0, "Primeira frase."),
            cue(1.2, 2.0, "Segunda frase."),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras.len(), 1);
    }

    #[test]
    fn breaks_paragraph_when_over_seven_hundred_chars() {
        let long = "palavra ".repeat(100);
        let cues = vec![
            cue(0.0, 1.0, &format!("{}.", long)),
            cue(1.1, 2.0, "Curta."),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn emits_a_heading_every_ten_minutes() {
        let cues = vec![cue(0.0, 1.0, "Inicio."), cue(650.0, 651.0, "Depois.")];
        let headings: Vec<_> = reflow(&cues)
            .into_iter()
            .filter(|b| matches!(b, Block::Heading { .. }))
            .collect();
        assert_eq!(headings.len(), 2);
    }

    #[test]
    fn unterminated_trailing_cues_still_produce_a_paragraph() {
        // Production audio produced long stretches averaging 2.3 words per cue
        // with no sentence terminator at all. Nothing may be dropped.
        let cues = vec![
            cue(0.0, 0.5, "que tem toda"),
            cue(0.5, 1.0, "a legitimidade"),
            cue(1.0, 1.5, "para tal"),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras, vec!["que tem toda a legitimidade para tal"]);
    }

    #[test]
    fn empty_input_produces_no_blocks() {
        assert!(reflow(&[]).is_empty());
    }

    #[test]
    fn formats_timestamps_with_and_without_hours() {
        assert_eq!(format_timestamp(65.0), "1:05");
        assert_eq!(format_timestamp(3725.0), "1:02:05");
    }

    #[test]
    fn closes_sentence_with_terminator_followed_by_trailing_whitespace() {
        // Regression test: cue text from Whisper commonly has trailing whitespace
        // after sentence terminators. Bug: closing was checked on untrimmed text,
        // so "Primeira frase. " failed to close, merging with the next paragraph
        // even across long pauses. Fix checks closure on the same trimmed value.
        let cues = vec![
            cue(0.0, 1.0, "Primeira frase. "),
            cue(3.0, 4.0, "Segunda frase."),
        ];
        let paras = para_texts(&reflow(&cues));
        assert_eq!(paras.len(), 2);
    }

    fn para_texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}
