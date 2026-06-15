//! Parses the heading outline from Markdown source.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

/// A single heading in the document outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// Heading level, 1..=6.
    pub level: u8,
    /// Plain-text content of the heading (inline markup stripped).
    pub text: String,
}

/// Parses the Markdown source into an ordered list of headings.
///
/// Enumerates only Markdown headings (ATX/Setext) in document order; raw-HTML
/// `<h2>` blocks are not counted, so the indices line up one-to-one with the
/// `id="h{n}"` anchors the renderer page injects.
pub fn parse(source: &str) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut current: Option<(u8, String)> = None;
    for event in Parser::new(source) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((level_to_u8(level), String::new()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, text)) = current.as_mut() {
                    text.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = current.take() {
                    headings.push(Heading { level, text });
                }
            }
            _ => {}
        }
    }
    headings
}

fn level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_levels_and_text_in_order() {
        let md = "# Title\n\nbody\n\n## Setup\n\ntext\n\n### Deep\n";
        assert_eq!(
            parse(md),
            vec![
                Heading { level: 1, text: "Title".into() },
                Heading { level: 2, text: "Setup".into() },
                Heading { level: 3, text: "Deep".into() },
            ]
        );
    }

    #[test]
    fn strips_inline_markup_but_keeps_code_text() {
        let md = "# Hello `world` and **bold**\n";
        assert_eq!(parse(md), vec![Heading { level: 1, text: "Hello world and bold".into() }]);
    }

    #[test]
    fn ignores_raw_html_headings_to_preserve_index_alignment() {
        let md = "# Real\n\n<h2>Raw HTML heading</h2>\n\n## Also real\n";
        assert_eq!(
            parse(md),
            vec![
                Heading { level: 1, text: "Real".into() },
                Heading { level: 2, text: "Also real".into() },
            ]
        );
    }

    #[test]
    fn empty_source_has_no_headings() {
        assert!(parse("").is_empty());
    }
}
