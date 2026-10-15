//! The diff pane: what changed in a project.

use unicode_width::UnicodeWidthChar;
use winit::keyboard::NamedKey;

use super::{Hit, Hits, PAD, PaneLayout, Ui, pane, theme};
use crate::action::Action;
use crate::core::DiffView;
use crate::core::git::Kind;
use crate::render::font::{BOLD, UI};
use crate::render::{Canvas, Rect};

/// Most marks that are drawn onto one sign of a line of the changes.
const MARKS: usize = 16;

/// Rows the pane shows at once.
pub fn rows(ui: &Ui, l: &PaneLayout) -> usize {
    ((l.area.h - l.header.bottom()) / ui.cell_h).floor().max(1.0) as usize
}

/// Scrolls with the keyboard. Returns whether the key meant something.
pub fn key(view: &mut DiffView, key: NamedKey, page: usize) -> bool {
    let page = page as isize;
    view.scroll_by(match key {
        NamedKey::ArrowUp => -1,
        NamedKey::ArrowDown => 1,
        NamedKey::PageUp => -page,
        NamedKey::PageDown | NamedKey::Space => page,
        NamedKey::Home => isize::MIN / 2,
        NamedKey::End => isize::MAX / 2,
        _ => return false,
    });
    true
}

pub fn draw(cv: &mut Canvas, ui: &Ui, view: &DiffView, area: Rect, hits: &mut Hits) {
    if view.lines.is_empty() {
        let text = if view.loaded { "No changes" } else { "Reading changes…" };
        return pane::centered(cv, ui, area, text, theme::FAINT);
    }
    let (cw, ch, px) = (ui.cell_w, ui.cell_h, ui.mono_px);
    let pad = ui.px(PAD);
    // Two line number columns, then the sign, then the text.
    let numbers = 5.0 * cw;
    let text_x = area.x + pad + 2.0 * numbers + 2.0 * cw;
    let columns = ((area.right() - pad - text_x) / cw).floor().max(0.0) as usize;
    let rows = (area.h / ch).floor() as usize;

    for (i, line) in view.lines.iter().enumerate().skip(view.scroll).take(rows) {
        let row = Rect::new(area.x, area.y + (i - view.scroll) as f32 * ch, area.w, ch);
        let base = row.y + ui.cell_base;
        // A character takes the cells it is wide: two for most of the
        // Chinese, Japanese and Korean ones. A mark takes none and goes
        // onto the character before it. Nothing is drawn past `columns`.
        let mono = |cv: &mut Canvas, text: &str, x: f32, color, columns: usize| {
            let (mut cell, mut marks) = (0, 0);
            for c in text.chars() {
                let wide = c.width().unwrap_or(0);
                if cell + wide > columns {
                    break;
                }
                // Marks take no column, so the columns do not bound
                // them: a line of nothing else would be drawn whole,
                // a thousand of them on one sign.
                marks = if wide == 0 { marks + 1 } else { 0 };
                if marks > MARKS {
                    continue;
                }
                let at = if wide == 0 { cell.saturating_sub(1) } else { cell };
                cv.glyph(c, 0, px, x + at as f32 * cw, base, color);
                cell += wide;
            }
        };
        let number = |cv: &mut Canvas, n: u32, column: f32| {
            if n > 0 {
                let text = n.to_string();
                let x = area.x + pad + column * numbers + (4usize.saturating_sub(text.len())) as f32 * cw;
                mono(cv, &text, x, theme::FAINT, usize::MAX);
            }
        };
        match line.kind {
            Kind::File | Kind::Gone => {
                // The row of a file leads to the file itself, where there
                // is one to show: not for a file that was deleted or
                // lies outside the folder of the project.
                let show = view.leads(i).map(|check| Action::DiffFile(i as u32, check));
                let hot = show.is_some_and(|show| ui.hot(show));
                cv.rect(row, if hot { theme::ACTIVE } else { theme::HOVER });
                if let Some(show) = show {
                    hits.push((row, Hit::Do(show)));
                }
                let base = ui.baseline(row);
                let mut x = area.x + pad;
                x += cv.text(&line.text, UI | BOLD, ui.text_px, x, base, theme::TEXT, area.w * 0.7)
                    + ui.px(12.0);
                x += cv.text(&format!("+{}", line.new), UI, ui.text_px, x, base, theme::ADDED, area.w)
                    + ui.px(8.0);
                cv.text(&format!("-{}", line.old), UI, ui.text_px, x, base, theme::REMOVED, area.w);
            }
            Kind::Hunk => {
                // What git names as context, usually the enclosing function.
                mono(cv, &format!("@@ {}", line.text), text_x, theme::FAINT, columns);
            }
            Kind::Note => {
                cv.text(
                    &line.text,
                    UI,
                    ui.text_px,
                    area.x + pad,
                    ui.baseline(row),
                    theme::DIM,
                    area.w - 2.0 * pad,
                );
            }
            Kind::Add | Kind::Del | Kind::Context => {
                let (background, sign, color) = match line.kind {
                    Kind::Add => (Some(theme::ADDED_BG), "+", theme::ADDED),
                    Kind::Del => (Some(theme::REMOVED_BG), "-", theme::REMOVED),
                    _ => (None, "", theme::DIM),
                };
                if let Some(background) = background {
                    cv.rect(row, background);
                }
                number(cv, line.old, 0.0);
                number(cv, line.new, 1.0);
                mono(cv, sign, text_x - 2.0 * cw, color, usize::MAX);
                let color = if line.kind == Kind::Context { theme::DIM } else { theme::TEXT };
                mono(cv, &line.text, text_x, color, columns);
            }
        }
    }
}
