//! A text field: one line with the caret at its end.

use super::{Ui, theme};
use crate::render::font::UI;
use crate::render::{Canvas, Color, Rect};

/// Longest text a field takes, in characters.
const MAX: usize = 512;

/// Adds what was typed or pasted to the text of a field: nothing that is
/// not to be seen, and no more than a field takes.
pub fn insert(text: &mut String, typed: &str) {
    let room = MAX.saturating_sub(text.chars().count());
    text.extend(typed.chars().filter(|c| !c.is_control()).take(room));
}

/// Text with a caret behind it. A text too long for the field shows its end.
pub fn draw(cv: &mut Canvas, ui: &Ui, field: Rect, x: f32, text: &str, placeholder: &str, color: Color) {
    let px = ui.text_px;
    let base = ui.baseline(field);
    let max = field.right() - x - ui.px(10.0);
    // Walk back from the end until the field is full.
    let mut from = text.len();
    let mut width = 0.0;
    for (at, c) in text.char_indices().rev() {
        width += cv.text_width(c.encode_utf8(&mut [0; 4]), UI, px);
        if width > max {
            break;
        }
        from = at;
    }
    let width = cv.text(&text[from..], UI, px, x, base, color, f32::MAX);
    let height = (ui.text.ascent + ui.text.descent).round();
    let caret = Rect::new(x + width + 1.0, base - ui.text.ascent.round(), ui.line(), height);
    cv.rect(caret, theme::TEXT);
    if text.is_empty() {
        cv.text(placeholder, UI, px, x + ui.px(6.0), base, theme::FAINT, max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_takes_what_is_to_be_seen_up_to_its_limit() {
        let mut text = String::from("a");
        insert(&mut text, "b\nc\u{7}\td");
        assert_eq!(text, "abcd");
        insert(&mut text, &"é".repeat(2 * MAX));
        assert_eq!(text.chars().count(), MAX);
        insert(&mut text, "x");
        assert_eq!(text.chars().count(), MAX);
    }
}
