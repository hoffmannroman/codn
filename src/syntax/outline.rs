//! Markdown read into what a text is made of. The only place that knows
//! `pulldown-cmark`, and it runs in the process for the colors alone: the
//! crate ends the process it runs in where something it holds true of
//! itself is not, and a release build ends on a panic with every session.

use pulldown_cmark::{Alignment, BlockQuoteKind, CowStr, Event, HeadingLevel, Options, Parser, Tag};

use super::wire::{self, OUTLINE_MAX};
use crate::core::preview::{Align, COLUMNS_WIRE, ENTRIES_MAX, Entry, WORD_MAX, What, Why, Word};

/// What the crate is asked to read beside plain Markdown: tables, text
/// that is struck through, tasks, a head between dashes, the words
/// over a quote, links in two brackets. Footnotes too, though they are
/// not drawn as such: read without them, the mark of one is a link to
/// the first word of its note. Not math, which stands as it is written.
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_GFM
        | Options::ENABLE_WIKILINKS
}

/// A piece of text the crate hands over: where it stands in the text, if
/// it is a piece of it, else its characters, as many as an entry carries.
fn place(text: &str, word: &CowStr) -> Word {
    if let CowStr::Borrowed(part) = word {
        let from = (part.as_ptr() as usize).checked_sub(text.as_ptr() as usize);
        if let Some(from) = from.filter(|from| from.saturating_add(part.len()) <= text.len()) {
            return Word::Here(from as u32, (from + part.len()) as u32);
        }
    }
    let mut end = word.len().min(WORD_MAX);
    while !word.is_char_boundary(end) {
        end -= 1;
    }
    Word::Said(word[..end].into())
}

/// What a text of Markdown is made of, entry by entry, or why that is
/// not said: it is no text, or there is more to it than is taken.
pub fn read(text: &[u8]) -> Result<Vec<Entry>, Why> {
    let text = std::str::from_utf8(text).map_err(|_| Why::NotText)?;
    let (mut entries, mut weight) = (Vec::new(), 64);
    let mut put = |what: What, from: usize, to: usize| {
        let entry = Entry { what, from: from as u32, to: to as u32 };
        weight += wire::weight(&entry);
        if entries.len() == ENTRIES_MAX || weight > OUTLINE_MAX {
            return Err(Why::Large);
        }
        entries.push(entry);
        Ok(())
    };
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        let from = range.start.min(text.len());
        let to = range.end.clamp(from, text.len());
        let here = Word::Here(from as u32, to as u32);
        // The note of a footnote stands under its name, as a paragraph
        // of its own: the name is not among what the crate hands over.
        if let Event::Start(Tag::FootnoteDefinition(name)) = &event {
            let name: String = name.chars().take(64).collect();
            put(What::Paragraph, from, to)?;
            put(What::Text(Word::Said(format!("[^{name}]:").into())), from, to)?;
            put(What::End, from, to)?;
        }
        let what = match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => What::Paragraph,
                Tag::Heading { level, .. } => What::Heading(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                }),
                Tag::BlockQuote(kind) => What::Quote(match kind {
                    None => 0,
                    Some(BlockQuoteKind::Note) => 1,
                    Some(BlockQuoteKind::Tip) => 2,
                    Some(BlockQuoteKind::Important) => 3,
                    Some(BlockQuoteKind::Warning) => 4,
                    Some(BlockQuoteKind::Caution) => 5,
                }),
                Tag::CodeBlock(_) => What::Code,
                Tag::HtmlBlock => What::Html,
                Tag::List(first) => What::List(first),
                Tag::Item => What::Item,
                Tag::Table(columns) => {
                    let align = |column: &Alignment| match column {
                        Alignment::None => Align::None,
                        Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    };
                    What::Table(columns.iter().take(COLUMNS_WIRE).map(align).collect())
                }
                Tag::TableHead => What::Head,
                Tag::TableRow => What::Row,
                Tag::TableCell => What::Cell,
                Tag::MetadataBlock(_) => What::Meta,
                Tag::Emphasis => What::Emphasis,
                Tag::Strong => What::Strong,
                Tag::Strikethrough => What::Strike,
                Tag::Link { dest_url, .. } => What::Link(place(text, &dest_url)),
                Tag::Image { dest_url, .. } => What::Image(place(text, &dest_url)),
                _ => What::Other,
            },
            Event::End(_) => What::End,
            Event::Text(word) | Event::Html(word) => What::Text(place(text, &word)),
            Event::Code(word) => What::CodeSpan(place(text, &word)),
            Event::InlineHtml(word) => What::InlineHtml(place(text, &word)),
            Event::SoftBreak => What::Soft,
            Event::HardBreak => What::Hard,
            Event::Rule => What::Rule,
            Event::TaskListMarker(checked) => What::Task(checked),
            // What is not read as anything of its own stands as it is
            // written: a footnote, math.
            _ => What::Text(here),
        };
        put(what, from, to)?;
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::preview::Meant;

    /// What the entries of a text are, in a few signs each, with the
    /// words they name.
    fn told(text: &str) -> String {
        let entries = read(text.as_bytes()).unwrap();
        let word = |word: &Word| match word {
            Word::Here(from, to) => text[*from as usize..*to as usize].to_owned(),
            Word::Said(said) => format!("={said}"),
        };
        let told: Vec<String> = entries
            .iter()
            .map(|entry| match &entry.what {
                What::Paragraph => "p".into(),
                What::Heading(level) => format!("h{level}"),
                What::Quote(kind) => format!("quote{kind}"),
                What::Code => "code".into(),
                What::Html => "html".into(),
                What::List(first) => {
                    format!("list{}", first.map_or(String::new(), |first| first.to_string()))
                }
                What::Item => "item".into(),
                What::Table(columns) => format!("table{}", columns.len()),
                What::Head => "head".into(),
                What::Row => "row".into(),
                What::Cell => "cell".into(),
                What::Meta => "meta".into(),
                What::Emphasis => "em".into(),
                What::Strong => "strong".into(),
                What::Strike => "strike".into(),
                What::Link(to) => format!("link({})", word(to)),
                What::Image(to) => format!("image({})", word(to)),
                What::Other => "other".into(),
                What::End => "/".into(),
                What::Text(text) => format!("{:?}", word(text)),
                What::CodeSpan(text) => format!("`{}`", word(text)),
                What::InlineHtml(text) => format!("<{}>", word(text)),
                What::Soft => "soft".into(),
                What::Hard => "hard".into(),
                What::Rule => "rule".into(),
                What::Task(checked) => format!("task{}", *checked as u8),
            })
            .collect();
        // What is read fits the text it was read from.
        assert!(Meant::new(text.as_bytes().to_vec(), entries).is_some(), "{text:?}");
        told.join(" ")
    }

    #[test]
    fn a_text_of_markdown_is_told_entry_by_entry() {
        assert_eq!(told("# One\n\ntwo *three*\nfour\n"), r#"h1 "One" / p "two " em "three" / soft "four" /"#);
        assert_eq!(
            told("- a\n- [x] b\n\n1. c\n"),
            r#"list item "a" / item task1 "b" / / list1 item "c" / /"#
        );
        assert_eq!(told("> [!NOTE]\n> a `b`\n"), r#"quote1 p "a " `b` / /"#);
        assert_eq!(told("```rust\nlet a;\n\n```\n\n---\n"), r#"code "let a;\n\n" / rule"#);
        assert_eq!(
            told("|a|b|\n|-|:-:|\n|~~c~~|d|\n"),
            r#"table2 head cell "a" / cell "b" / / row cell strike "c" / / cell "d" / / /"#
        );
        assert_eq!(told("---\ntitle: x\n---\n\ny\n"), r#"meta "title: x\n" / p "y" /"#);
        assert_eq!(
            told("[a](b.md#c) ![d](e.png) <https://f.g> [[h]]\n"),
            r#"p link(b.md#c) "a" / " " image(e.png) "d" / " " link(https://f.g) "https://f.g" / " " link(h) "h" / /"#
        );
        assert_eq!(told("<div>\nx\n</div>\n\na <b>c</b>  \nd\n"), {
            r#"html "<div>\n" "x\n" "</div>\n" / p "a " <<b>> "c" <</b>> hard "d" /"#
        });
    }

    #[test]
    fn what_is_written_elsewhere_or_named_by_a_sign_is_said_and_not_placed() {
        assert_eq!(told("a &amp; b\n"), r#"p "a " "=&" " b" /"#);
        assert_eq!(told("[a][r]\n\n[r]: https://x.y\n"), r#"p link(https://x.y) "a" / /"#);
        // The mark of a footnote stands as it is written, and its note
        // under its name.
        assert_eq!(told("a[^1]\n\n[^1]: b\n"), r#"p "a" "[^1]" / p "=[^1]:" / other p "b" / /"#);
        assert_eq!(told("$a$\n"), r#"p "$a$" /"#);
    }

    #[test]
    fn what_is_no_text_or_has_too_much_to_it_is_not_told() {
        assert_eq!(read(b"a\xffb"), Err(Why::NotText));
        assert_eq!(read(b""), Ok(Vec::new()));
        // An entry for every sign.
        let many = "[".repeat(ENTRIES_MAX);
        assert_eq!(read(many.as_bytes()), Err(Why::Large));
        // An address longer than an entry carries is cut between two signs.
        let long = format!("[a][r]\n\n[r]: <{}>\n", "ä".repeat(WORD_MAX));
        let entries = read(long.as_bytes()).unwrap();
        let What::Link(Word::Here(from, to)) = &entries[1].what else { panic!("{:?}", entries[1]) };
        assert_eq!((to - from) as usize, 2 * WORD_MAX, "it stands in the text, and is placed there");
        let cut = place("", &CowStr::Boxed("ä".repeat(WORD_MAX).into()));
        assert_eq!(cut, Word::Said("ä".repeat(WORD_MAX / 2).into()));
    }

    #[test]
    fn texts_made_to_harm_are_read_or_refused_and_end_nothing() {
        let n = 20_000;
        let texts = [
            "> ".repeat(n) + "a\n",
            (0..500).map(|at| format!("{}- a\n", "  ".repeat(at))).collect(),
            "[a](".repeat(n),
            "*a ".repeat(n),
            "**a*b ".repeat(n),
            (1..500).map(|at| "`".repeat(at) + " ").collect(),
            format!("{}\n{}\n{}\n", "|a".repeat(2_000), "|-".repeat(2_000), "|b".repeat(2_000)),
            format!("|a|b|\n|-|-|\n{}", "|c|d|\n".repeat(n)),
            "<a ".repeat(n),
            "&amp;&#x41;&bogus;".repeat(n),
            "_".repeat(n) + "a" + &"_".repeat(n),
            "[[".repeat(n),
            "\u{0}\u{fffd}\u{202e}\r".repeat(n),
        ];
        for text in texts {
            if let Ok(entries) = read(text.as_bytes()) {
                assert!(Meant::new(text.into_bytes(), entries).is_some());
            }
        }
    }
}
