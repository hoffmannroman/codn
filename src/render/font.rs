//! Font lookup through fontconfig and rasterization through swash.
//!
//! fontconfig is asked through its command line tools, never while drawing:
//! the primary faces are resolved at startup, and one more query on a worker
//! thread lists every installed font with the characters it covers. Looking
//! for a font that has a missing character is then a search in memory.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform};
use swash::{CacheKey, FontRef, Style, tag_from_bytes};

use super::fontconfig::{self, Candidate, Match};
use crate::log::log;

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
/// Proportional interface font instead of the terminal font.
pub const UI: u8 = 4;
/// Most characters whose fallback font is remembered.
const MAX_FALLBACK: usize = 1 << 16;
/// The largest size an image of a sign is made at. A size comes from the
/// scale of the window, which the window system names: at a size of
/// nothing or of no number a font draws its signs as large as they were
/// designed, a megabyte each, and no atlas takes what is larger than this.
pub const MAX_PX: f32 = 2048.0;
/// The largest font file that is read, and the most that all font files
/// together may hold in memory. A file of fonts for Chinese, Japanese and
/// Korean has some tens of megabytes; a program that prints a sign of
/// every script there is makes codn read a file for each.
const MAX_FILE: u64 = 64 << 20;
const MAX_HELD: usize = 256 << 20;

/// Used when fontconfig's tools are missing.
const FALLBACK_FILES: &[&str] = &[
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
];

struct Face {
    /// The font file, read and not mapped. A file that is mapped ends
    /// the app when it is cut while a sign is drawn from it, which is
    /// what writing a new font over an old one in place begins with.
    data: Arc<[u8]>,
    offset: u32,
    key: CacheKey,
    /// The file is regular weight, bold is synthesized.
    embolden: bool,
    /// The file is upright, italic is synthesized.
    slant: bool,
    /// Variable font that needs its weight axis set for bold.
    var_bold: bool,
}

impl Face {
    fn font(&self) -> FontRef<'_> {
        FontRef { data: &self.data, offset: self.offset, key: self.key }
    }
}

/// Whether every table a font names lies in its file. A file that was
/// cut off begins as a font and is taken for one, and what is read of a
/// table that is not there is nothing: no sign has a size then.
fn whole(data: &[u8], offset: u32) -> bool {
    let number = |at: usize, bytes: usize| {
        let field = data.get(at..at.checked_add(bytes)?)?;
        Some(field.iter().fold(0usize, |number, byte| number << 8 | *byte as usize))
    };
    let lies = |table: usize| {
        let record = (offset as usize).checked_add(12)?.checked_add(table.checked_mul(16)?)?;
        let end = number(record + 8, 4)?.checked_add(number(record + 12, 4)?)?;
        Some(end <= data.len())
    };
    number(offset as usize + 4, 2).is_some_and(|tables| (0..tables).all(|table| lies(table) == Some(true)))
}

pub struct Raster {
    pub left: i32,
    pub top: i32,
    pub w: u32,
    pub h: u32,
    pub color: bool,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    /// Advance of `0`, the cell width for a monospace face.
    pub advance: f32,
}

pub struct Fonts {
    mono_family: String,
    ui_family: String,
    files: HashMap<PathBuf, Arc<[u8]>>,
    /// The bytes of all files that were read.
    held: usize,
    faces: Vec<Face>,
    by_file: HashMap<(PathBuf, u32, u8), Option<usize>>,
    styled: [Option<usize>; 8],
    /// Every installed font in fontconfig's order of preference, once the
    /// worker delivered it.
    candidates: Vec<Candidate>,
    pending: Option<Receiver<Vec<Candidate>>>,
    fallback: HashMap<char, Option<usize>>,
    /// Counts the arrivals of the fallback list. Whoever remembers that a
    /// character has no glyph compares this to know when to ask again.
    pub generation: u32,
    ctx: ScaleContext,
}

impl Fonts {
    /// Resolves the fonts to use. Blocks for a few fontconfig calls, so it
    /// is meant to run beside the creation of the window. `ready` is called
    /// from a worker once fallback fonts can be looked up.
    pub fn load(
        mono_family: &str,
        ui_family: &str,
        ready: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(fontconfig::candidates());
            ready();
        });
        let begun = Instant::now();
        let (mono, ui) = std::thread::scope(|scope| {
            let ui = scope.spawn(|| fontconfig::installed(ui_family, "sans-serif"));
            (
                fontconfig::installed(mono_family, "monospace"),
                ui.join().unwrap_or_else(|_| "sans-serif".into()),
            )
        });
        // The window waits for this. A fontconfig that did not answer the
        // first request within its limit is not asked again: every further
        // request would wait as long, and they add up to half a minute.
        let answers = begun.elapsed() < fontconfig::LOOKUP;
        let mut fonts = Self {
            mono_family: mono,
            ui_family: ui,
            files: HashMap::new(),
            held: 0,
            faces: Vec::new(),
            by_file: HashMap::new(),
            styled: [None; 8],
            candidates: Vec::new(),
            pending: Some(rx),
            fallback: HashMap::new(),
            generation: 0,
            ctx: ScaleContext::new(),
        };
        // Every style that is drawn, looked up side by side. A style left
        // out here would start `fc-match` in the middle of a frame.
        let patterns = [0, BOLD, ITALIC, BOLD | ITALIC, UI, UI | BOLD, UI | ITALIC, UI | BOLD | ITALIC]
            .map(|style| (style, fonts.pattern(style)));
        let found: Vec<(u8, Option<Match>)> = std::thread::scope(|scope| {
            let lookups: Vec<_> = patterns
                .iter()
                .map(|(style, pattern)| {
                    let ask = move || fontconfig::best(pattern);
                    (*style, answers.then(|| scope.spawn(ask)))
                })
                .collect();
            let answer = |lookup: std::thread::ScopedJoinHandle<_>| lookup.join().ok().flatten();
            lookups.into_iter().map(|(style, lookup)| (style, lookup.and_then(answer))).collect()
        });
        if found.iter().all(|(_, font)| font.is_none()) {
            log("fontconfig does not answer in time, using a font file that is usually there");
        }
        for (style, font) in found {
            // The regular style comes first, the others can fall back on it.
            let face = font.and_then(|m| fonts.face(&m, style)).filter(|face| fonts.sets(*face));
            fonts.styled[style as usize] = face.or_else(|| fonts.spare_face(style)).or(fonts.styled[0]);
        }
        if fonts.styled[0].is_none() {
            return Err("no usable monospace font found (is fontconfig installed?)".into());
        }
        Ok(fonts)
    }

    /// Takes over the list of fallback fonts once the worker has it.
    /// Returns whether characters that had no glyph may have one now.
    pub fn poll(&mut self) -> bool {
        let Some(list) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) else { return false };
        self.candidates = list;
        self.pending = None;
        self.fallback.clear();
        self.generation += 1;
        true
    }

    /// The fontconfig pattern for a style of the terminal or interface font.
    fn pattern(&self, style: u8) -> String {
        let mut pattern = if style & UI != 0 { self.ui_family.clone() } else { self.mono_family.clone() };
        if style & BOLD != 0 {
            pattern.push_str(":weight=bold");
        }
        if style & ITALIC != 0 {
            pattern.push_str(":slant=italic");
        }
        pattern
    }

    /// Reads a font file, or returns what was read of it before. Nothing
    /// for a file that is larger than is read, or for which no room is
    /// left: its signs are then drawn by no font.
    fn file(&mut self, path: &PathBuf) -> Option<Arc<[u8]>> {
        if let Some(data) = self.files.get(path) {
            return Some(data.clone());
        }
        let file = std::fs::File::open(path).ok()?;
        let size = file.metadata().ok().filter(|file| file.is_file())?.len();
        if size > MAX_FILE || size as usize > MAX_HELD.saturating_sub(self.held) {
            return None;
        }
        // The file may grow while it is read: no more is taken than it
        // was held against.
        let mut data = Vec::with_capacity(size as usize);
        file.take(size).read_to_end(&mut data).ok()?;
        self.held += data.len();
        let data: Arc<[u8]> = data.into();
        self.files.insert(path.clone(), data.clone());
        Some(data)
    }

    /// Loads a face for a style, or returns the one loaded before.
    fn face(&mut self, m: &Match, style: u8) -> Option<usize> {
        let want = style & (BOLD | ITALIC);
        let key = (m.file.clone(), m.index, want);
        if let Some(&face) = self.by_file.get(&key) {
            return face;
        }
        let face = self.file(&m.file).and_then(|data| {
            // The high bits of a fontconfig index name a variable instance.
            let font = FontRef::from_index(&data, (m.index & 0xffff) as usize)?;
            let (offset, key) = (font.offset, font.key);
            if !whole(&data, offset) {
                return None;
            }
            // Ask the font what it is. fontconfig reports the style it was
            // asked for, even when it only found the regular file.
            let attributes = font.attributes();
            let fake_bold = want & BOLD != 0 && attributes.weight().0 < 600;
            let variable = font.variations().any(|v| v.tag() == tag_from_bytes(b"wght"));
            let slant = want & ITALIC != 0 && attributes.style() == Style::Normal;
            self.faces.push(Face {
                data,
                offset,
                key,
                embolden: fake_bold && !variable,
                slant,
                var_bold: fake_bold && variable,
            });
            Some(self.faces.len() - 1)
        });
        self.by_file.insert(key, face);
        face
    }

    /// The primary face for a style, falling back to the regular face.
    fn styled_face(&mut self, style: u8) -> Option<usize> {
        let slot = (style & 7) as usize;
        if self.styled[slot].is_none() {
            let mut face = fontconfig::best(&self.pattern(style)).and_then(|m| self.face(&m, style));
            face = face.filter(|face| self.sets(*face)).or_else(|| self.spare_face(style));
            if face.is_none() && style != 0 {
                face = self.styled_face(0);
            }
            self.styled[slot] = face;
        }
        self.styled[slot]
    }

    /// A face for a style of the terminal font that needs no fontconfig:
    /// from a font file that is usually there.
    fn spare_face(&mut self, style: u8) -> Option<usize> {
        if style & UI != 0 {
            return None;
        }
        FALLBACK_FILES.iter().find_map(|f| {
            let m = Match { file: f.into(), index: 0, family: String::new() };
            self.face(&m, style).filter(|face| self.sets(*face))
        })
    }

    /// Whether a face can set text: its signs have a height, and `0` a
    /// width. A file that begins as a font and has lost its tables is
    /// taken for one, and says nothing of either: with it as the font of
    /// the terminal a cell has no size, and nothing is drawn anywhere.
    fn sets(&self, face: usize) -> bool {
        let font = self.faces[face].font();
        let m = font.metrics(&[]).scale(16.0);
        let advance = font.glyph_metrics(&[]).scale(16.0).advance_width(font.charmap().map('0'));
        [advance, m.ascent + m.descent].iter().all(|size| size.is_finite() && *size >= 1.0)
    }

    /// Finds the face and glyph id that render `c` in the given style.
    fn resolve(&mut self, c: char, style: u8) -> Option<(usize, u16)> {
        let primary = self.styled_face(style)?;
        let gid = self.faces[primary].font().charmap().map(c);
        if gid != 0 {
            return Some((primary, gid));
        }
        let face = match self.fallback.get(&c) {
            Some(&face) => face,
            None => {
                self.poll();
                if self.pending.is_some() {
                    // The list is on its way. Not remembered as missing.
                    return None;
                }
                // Emoji look for a color font first, everything else takes
                // the first font in fontconfig's order that has it.
                let emoji = c as u32 >= 0x1f000;
                let covering = || self.candidates.iter().filter(|candidate| candidate.covers(c));
                let pick =
                    covering().find(|candidate| candidate.color == emoji).or_else(|| covering().next());
                let font = pick.map(|candidate| candidate.font.clone());
                let face = font
                    .and_then(|font| self.face(&font, 0))
                    .filter(|&face| self.faces[face].font().charmap().map(c) != 0);
                if self.fallback.len() >= MAX_FALLBACK {
                    self.fallback.clear();
                }
                self.fallback.insert(c, face);
                face
            }
        }?;
        Some((face, self.faces[face].font().charmap().map(c)))
    }

    pub fn metrics(&mut self, style: u8, px: f32) -> Metrics {
        let Some(face) = self.styled_face(style) else { return Metrics::default() };
        let font = self.faces[face].font();
        let m = font.metrics(&[]).scale(px);
        let advance = font.glyph_metrics(&[]).scale(px).advance_width(font.charmap().map('0'));
        Metrics { ascent: m.ascent, descent: m.descent, leading: m.leading, advance }
    }

    /// How far `c` moves the pen, without an image of it: what
    /// `rasterize` says of it, for whoever only measures.
    pub fn advance(&mut self, c: char, style: u8, px: f32) -> Option<f32> {
        if !(px > 0.0 && px <= MAX_PX) {
            return None;
        }
        let (face, gid) = self.resolve(c, style)?;
        Some(self.faces[face].font().glyph_metrics(&[]).scale(px).advance_width(gid))
    }

    /// Rasterizes `c`, returning the image (if it has one) and its advance.
    pub fn rasterize(&mut self, c: char, style: u8, px: f32) -> Option<(Option<Raster>, f32)> {
        if !(px > 0.0 && px <= MAX_PX) {
            return None;
        }
        let (face, gid) = self.resolve(c, style)?;
        let face = &self.faces[face];
        let font = face.font();
        let weight = [("wght", 700.0)];
        let variations: &[(&str, f32)] = if face.var_bold { &weight } else { &[] };
        let advance = font.glyph_metrics(&[]).scale(px).advance_width(gid);
        let mut scaler =
            self.ctx.builder(font).size(px).hint(true).variations(variations.iter().copied()).build();
        let mut render = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
            Source::Bitmap(StrikeWith::BestFit),
        ]);
        render.format(Format::Alpha);
        if face.embolden {
            render.embolden(px / 28.0);
        }
        if face.slant {
            render.transform(Some(Transform::skew(Angle::from_degrees(12.0), Angle::ZERO)));
        }
        let Some(image) = render.render(&mut scaler, gid) else { return Some((None, advance)) };
        let p = image.placement;
        if p.width == 0 || p.height == 0 {
            return Some((None, advance));
        }
        let color = image.content == Content::Color;
        let mut data = image.data;
        // The atlas stores premultiplied alpha. Layered color outlines come
        // out of swash that way, color bitmaps do not.
        if matches!(image.source, Source::ColorBitmap(_)) {
            for px in data.as_chunks_mut::<4>().0 {
                let a = px[3] as u32;
                for v in &mut px[..3] {
                    *v = (*v as u32 * a / 255) as u8;
                }
            }
        }
        Some((Some(Raster { left: p.left, top: p.top, w: p.width, h: p.height, color, data }), advance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fonts with one file as the font of the terminal, and no other.
    /// Nothing if the file is not taken as that.
    fn of_file(file: PathBuf) -> Option<Fonts> {
        let mut fonts = Fonts {
            mono_family: String::new(),
            ui_family: String::new(),
            files: HashMap::new(),
            held: 0,
            faces: Vec::new(),
            by_file: HashMap::new(),
            styled: [None; 8],
            candidates: Vec::new(),
            pending: None,
            fallback: HashMap::new(),
            generation: 0,
            ctx: ScaleContext::new(),
        };
        let face = fonts.face(&Match { file, index: 0, family: String::new() }, 0)?;
        fonts.sets(face).then(|| {
            fonts.styled = [Some(face); 8];
            fonts
        })
    }

    /// A font file of this machine. Where there is none, nothing can be
    /// held against one: a machine that only builds has none.
    fn installed() -> Option<PathBuf> {
        FALLBACK_FILES.iter().map(PathBuf::from).find(|file| file.exists())
    }

    /// A folder of the test's own.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codn-font-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_image_is_made_at_a_size_that_is_none() {
        let Some(mut fonts) = installed().and_then(of_file) else { return };
        assert!(
            fonts.rasterize('a', 0, 14.0).is_some_and(|(image, advance)| image.is_some() && advance > 0.0)
        );
        for px in [0.0, -5.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, MAX_PX * 2.0, f32::MAX] {
            assert!(fonts.rasterize('a', 0, px).is_none(), "an image at {px}");
            assert!(fonts.advance('a', 0, px).is_none(), "a width at {px}");
        }
    }

    #[test]
    fn a_font_file_that_is_cut_while_it_is_used_draws_on() {
        let Some(whole) = installed() else { return };
        let dir = scratch("cut");
        let file = dir.join("font.ttf");
        std::fs::copy(whole, &file).unwrap();
        let mut fonts = of_file(file.clone()).unwrap();
        assert!(fonts.rasterize('a', 0, 14.0).is_some_and(|(image, _)| image.is_some()));
        // What `cp new.ttf old.ttf` does first to a file that is there.
        std::fs::OpenOptions::new().write(true).truncate(true).open(&file).unwrap();
        let drawn = fonts.rasterize('b', 0, 14.0).is_some_and(|(image, _)| image.is_some());
        let _ = std::fs::remove_dir_all(dir);
        assert!(drawn);
    }

    #[test]
    fn a_file_that_has_lost_its_tables_is_not_taken_as_the_font() {
        let Some(whole) = installed() else { return };
        let dir = scratch("half");
        let bytes = std::fs::read(&whole).unwrap();
        let (half, empty) = (dir.join("half.ttf"), dir.join("empty.ttf"));
        std::fs::write(&half, &bytes[..bytes.len() / 2]).unwrap();
        std::fs::write(&empty, "").unwrap();
        let taken = [of_file(whole).is_some(), of_file(half).is_some(), of_file(empty).is_some()];
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(taken, [true, false, false]);
    }

    /// A text is measured without the images of its signs, and drawn with
    /// them: the two widths are one.
    #[test]
    fn a_sign_is_as_wide_measured_as_it_is_drawn() {
        // Where no font is found there is nothing to hold this against:
        // a machine that only builds has none.
        let Ok(mut fonts) = Fonts::load("", "", || {}) else { return };
        let signs = ['a', 'W', '\u{e4}', ' ', '\u{301}', '\u{4e2d}', '\u{10ffff}'];
        for (c, style, px) in signs.into_iter().flat_map(|c| [(c, 0, 14.0), (c, 1, 20.5), (c, 3, 9.0)]) {
            let drawn = fonts.rasterize(c, style, px).map(|(_, advance)| advance);
            assert_eq!(fonts.advance(c, style, px), drawn, "{c:?} in style {style} at {px}");
        }
    }

    /// Letters with combining marks in the font named by `CODN_FONT`, or
    /// the system's, as they were placed before and as they are now. The
    /// rule itself is tested where it lives, in `canvas`.
    #[test]
    #[ignore = "writes native-size rasters of combining marks to look at"]
    fn combining_marks_to_look_at() {
        let family = std::env::var("CODN_FONT").unwrap_or_default();
        let mut fonts = Fonts::load(&family, "", || {}).unwrap();
        let dir = std::env::var_os("CODN_MARKS_OUT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("codn-combining-evidence"));
        std::fs::create_dir_all(&dir).unwrap();
        let px = 14.0;
        let cell = fonts.metrics(0, px).advance.round();
        let pairs =
            [('e', '\u{301}'), ('u', '\u{308}'), ('a', '\u{300}'), ('n', '\u{303}'), ('o', '\u{302}')];
        // Before: behind the character if the mark has no advance and
        // begins left of the pen, else where the character begins.
        fn before(left: f32, _: f32, mark: f32, advance: f32) -> f32 {
            if mark == 0.0 && left < 0.0 { advance } else { 0.0 }
        }
        fn after(left: f32, width: f32, _: f32, advance: f32) -> f32 {
            crate::render::canvas::mark_offset(left, width, advance)
        }
        type Rule = fn(f32, f32, f32, f32) -> f32;
        let rules: [(&str, Rule); 2] = [("combining-before.pgm", before), ("combining-after.pgm", after)];
        for (name, rule) in rules {
            let (w, h) = (16 + pairs.len() * cell as usize, 34usize);
            let mut image = vec![20u8; w * h];
            for (n, (base, mark)) in pairs.into_iter().enumerate() {
                let x = 8.0 + n as f32 * cell;
                let (base, advance) = fonts.rasterize(base, 0, px).unwrap();
                let (mark, mark_advance) = fonts.rasterize(mark, 0, px).unwrap();
                let (base, mark) = (base.unwrap(), mark.unwrap());
                let pen = x + rule(mark.left as f32, mark.w as f32, mark_advance, advance);
                for (raster, x) in [(&base, x), (&mark, pen)] {
                    let left = (x + raster.left as f32).round() as i32;
                    let top = 24 - raster.top;
                    for row in 0..raster.h as i32 {
                        for col in 0..raster.w as i32 {
                            let (x, y) = (left + col, top + row);
                            if x >= 0 && y >= 0 && x < w as i32 && y < h as i32 {
                                let ink = raster.data[row as usize * raster.w as usize + col as usize];
                                let pixel = &mut image[y as usize * w + x as usize];
                                *pixel = (*pixel).max(20 + (ink as u32 * 200 / 255) as u8);
                            }
                        }
                    }
                }
            }
            let mut output = format!("P5\n{w} {h}\n255\n").into_bytes();
            output.extend(image);
            std::fs::write(dir.join(name), output).unwrap();
        }
        eprintln!("combining marks: {} (font {family:?}, cell {cell})", dir.display());
    }
}
