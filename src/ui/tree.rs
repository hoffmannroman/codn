//! The column at the right: the files of a project as a tree.

use super::{HEADER, Hit, Hits, INSET, ROW, Ui, chrome, pane, theme};
use crate::action::Action;
use crate::core::Workspace;
use crate::core::files::Kind;
use crate::core::tree::Line;
use crate::render::font::{BOLD, UI};
use crate::render::{Canvas, Icon, Rect};

/// How much further in the entries of a folder stand than the folder.
const INDENT: f32 = 12.0;

/// The icon of a file by what its name says of it: the end of the name,
/// without regard to case. No file is opened to know.
fn icon_of(name: &str, kind: Kind) -> Icon {
    if kind == Kind::Link {
        return Icon::FileLink;
    }
    // A name that begins with its only dot has no ending: `.gitignore`.
    let ending = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map(|(_, ending)| ending);
    let Some(ending) = ending.map(str::to_ascii_lowercase) else { return Icon::File };
    match ending.as_str() {
        "json" | "jsonl" | "jsonc" | "ndjson" => Icon::FileJson,
        "md" | "mdx" | "markdown" => Icon::FileMarkdown,
        "pdf" => Icon::FilePdf,
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "rb" | "go" | "c" | "h" | "cc"
        | "cpp" | "cxx" | "hpp" | "java" | "kt" | "swift" | "cs" | "php" | "lua" | "sh" | "bash" | "zsh"
        | "fish" | "pl" | "sql" | "html" | "htm" | "css" | "scss" | "sass" | "less" | "vue" | "svelte"
        | "astro" | "qml" | "xml" | "toml" | "yaml" | "yml" | "nix" | "zig" | "el" | "vim" | "tex" => {
            Icon::FileCode
        }
        "txt" | "log" | "csv" | "tsv" | "ini" | "conf" | "cfg" | "env" | "lock" | "rst" | "org" => {
            Icon::FileText
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "ico" | "bmp" | "tif" | "tiff"
        | "heic" | "mp3" | "wav" | "ogg" | "flac" | "m4a" | "mp4" | "mov" | "mkv" | "webm" => Icon::FileMedia,
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "jar" => Icon::FileZip,
        "so" | "o" | "a" | "bin" | "exe" | "dll" | "dylib" | "wasm" | "class" | "pyc" | "db" | "sqlite"
        | "sqlite3" | "woff" | "woff2" | "ttf" | "otf" | "rgba" | "dat" | "npz" => Icon::FileBinary,
        _ => Icon::File,
    }
}

/// Where the rows begin: below the header, which holds what stands at
/// the top right of the window, and below the row that names the project.
fn top(ui: &Ui) -> f32 {
    ui.px(HEADER) + ui.px(ROW)
}

/// How many rows the column shows at once, parts of rows counted.
pub fn visible(ui: &Ui, area: Rect) -> f32 {
    ((area.h - top(ui)) / ui.px(ROW)).max(0.0)
}

/// Scrolls the rows by `lines` of the wheel, positive is up.
pub fn wheel(ui: &Ui, ws: &mut Workspace, area: Rect, lines: i32) {
    ws.tree.scroll_by(-(lines as f32) / 3.0, visible(ui, area));
}

/// Draws the column: the rows first, then the header that covers the ones
/// scrolled out under it.
pub fn draw(cv: &mut Canvas, ui: &Ui, ws: &Workspace, area: Rect, hits: &mut Hits) {
    let (row_h, px, inset) = (ui.px(ROW), ui.text_px, ui.px(INSET));
    let (top, icon) = (top(ui), ui.px(chrome::ICON));
    let tree = &ws.tree;
    cv.rect(area, theme::SIDEBAR);
    cv.rect(Rect::new(area.x - 1.0, 0.0, 1.0, area.h), theme::LINE);
    // A click between the rows means nothing, and not the pane behind.
    hits.push((area, Hit::None));

    let project = tree.project.and_then(|id| ws.project(id));
    let shown = ws.file_path();
    let scrolled = tree.scrolled(visible(ui, area));
    let first = scrolled.floor() as usize;
    let mut y = top - ((scrolled - first as f32) * row_h).round();
    for row in first..tree.rows() {
        if y >= area.h {
            break;
        }
        let place = Rect::new(area.x, y, area.w, row_h);
        let base = ui.baseline(place);
        match tree.line(row) {
            Some(Line::Entry { id, name, kind, open, broken, depth }) => {
                let click = Action::TreeRow(id);
                // The file that is shown is told by the surface of its row.
                if shown.is_some() && tree.path(row) == shown {
                    cv.rect(place, theme::ACTIVE);
                } else if ui.hot(click) {
                    cv.rect(place, theme::HOVER);
                }
                hits.push((place, Hit::Do(click)));
                let x = area.x + inset - ui.px(2.0) + depth as f32 * ui.px(INDENT);
                // Before a folder what opens it, before a file what kind
                // of file its name says it is, both in the same place.
                let (sign, tone) = if kind == Kind::Folder || open {
                    (if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::DIM)
                } else {
                    (icon_of(name, kind), if broken { theme::FAINT } else { theme::DIM })
                };
                chrome::symbol(cv, ui, sign, Rect::new(x, y, icon, row_h), tone);
                let color = match kind {
                    _ if broken => theme::FAINT,
                    Kind::Folder | Kind::File => theme::TEXT,
                    Kind::Link | Kind::Other => theme::DIM,
                };
                let text = x + icon + ui.px(4.0);
                let room = area.right() - inset - text;
                let used = cv.text(name, UI, px, text, base, color, room);
                if broken {
                    let rest = room - used - ui.px(8.0);
                    cv.text("leads nowhere", UI, px, text + used + ui.px(8.0), base, theme::FAINT, rest);
                }
            }
            Some(Line::Note { text, depth }) => {
                let x = area.x + inset + ui.px(chrome::ICON + 2.0) + depth as f32 * ui.px(INDENT);
                cv.text(text, UI, px, x, base, theme::FAINT, area.right() - inset - x);
            }
            None => {}
        }
        y += row_h;
    }
    if project.is_none() {
        let body = Rect::new(area.x, top, area.w, area.h - top);
        pane::centered(cv, ui, body, "No project", theme::FAINT);
    }

    // Above the rows: the header, which moves the window like every
    // header and is taken up by what stands at the top right of the
    // window, and below it the name of the project.
    let header = Rect::new(area.x, 0.0, area.w, ui.px(HEADER));
    let named = Rect::new(area.x, header.h, area.w, top - header.h);
    cv.rect(Rect::new(area.x, 0.0, area.w, top), theme::SIDEBAR);
    hits.push((header, Hit::Drag));
    hits.push((named, Hit::None));
    if let Some(project) = project {
        let x = area.x + inset;
        cv.text(&project.name, UI | BOLD, px, x, ui.baseline(named), theme::TEXT, area.w - 2.0 * inset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_has_the_icon_of_what_its_name_ends_in() {
        let of = |name: &str| icon_of(name, Kind::File);
        assert_eq!([of("main.rs"), of("App.TSX"), of("page.astro")], [Icon::FileCode; 3]);
        assert_eq!([of("package.json"), of("log.jsonl")], [Icon::FileJson; 2]);
        assert_eq!([of("README.md"), of("post.mdx")], [Icon::FileMarkdown; 2]);
        assert_eq!([of("shot.png"), of("mark.svg"), of("clip.MP4")], [Icon::FileMedia; 3]);
        assert_eq!(
            (of("a.pdf"), of("a.tar.gz"), of("codn.wasm")),
            (Icon::FilePdf, Icon::FileZip, Icon::FileBinary)
        );
        assert_eq!([of("notes.txt"), of("Cargo.lock"), of("local.env")], [Icon::FileText; 3]);
        // No ending, an ending nobody knows, and a name that is all ending.
        assert_eq!([of("Makefile"), of("a.xyz"), of(".gitignore"), of(".env"), of("a.")], [Icon::File; 5]);
        // A link is a link, whatever its name ends in.
        assert_eq!(icon_of("main.rs", Kind::Link), Icon::FileLink);
    }
}
