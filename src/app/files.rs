//! The column with the files of a project, and the pane that shows one:
//! what a click there leads to, what is asked of the disk, of git and of
//! the processes for the colors and for pictures and when, and what the
//! answers change. A file of Markdown is shown as it is meant from what
//! the process for the colors says it is made of.
//! Keys and the pointer in a file are in `editing`, saving it and the
//! question before its changes are left in `saving`.

use std::path::Path;
use std::time::Instant;

use super::{App, Asked, PICTURE_DELAY};
use crate::action::Action;
use crate::core::colors::End;
use crate::core::files::{Edition, Reply, Request, Stamp};
use crate::core::git::{self, Marks};
use crate::core::preview::Key;
use crate::core::tree::Clicked;
use crate::core::{Pane, ProjectId, Shown, Taken};
use crate::log::log;
use crate::picture;
use crate::picture::client::{self as pictures, Outcome};
use crate::picture::wire::{WHOLE, Wish};
use crate::syntax::{self, client as colors};
use crate::ui::{file, preview};

/// By which grammar a file is shown in colors, by its name. None, and it
/// has one color.
fn grammar_of(path: &Path) -> Option<&'static str> {
    path.file_name().and_then(syntax::language)
}

impl App {
    /// Shows the column with the files, or folds it away.
    pub(super) fn toggle_tree(&mut self) {
        // Where the column has no room, neither it nor its icon is
        // there, and nothing would show what the key did.
        if !self.ui.tree_fits(self.ws.panes().len()) {
            return;
        }
        self.ui.tree_open = !self.ui.tree_open;
        self.follow_tree();
        // The panes have another width now.
        self.fit();
        self.save_soon();
    }

    /// Folds the sidebar to a strip, or gives it its width back.
    pub(super) fn toggle_sidebar(&mut self) {
        self.ui.sidebar_folded = !self.ui.sidebar_folded;
        // The column with the files has more room now, or less.
        self.follow_tree();
        // The panes have another width now.
        self.fit();
        self.save_soon();
    }

    /// Brings the column in line with what is shown: it holds entries
    /// only while it is drawn, and shows the project that is worked in.
    /// Called between events, so it has to cost nothing when all is as
    /// it was.
    pub(super) fn follow_tree(&mut self) {
        if self.ui.tree(self.ws.panes().len()).is_none() {
            if self.ws.tree.unfolded() {
                self.ws.tree.fold();
            }
            return;
        }
        // The column follows the keyboard to another project, and stays
        // where it is while the keyboard is with nothing that has one.
        let active = self.ws.active_project();
        let moved = active.is_some() && active != self.tree_followed;
        if active.is_some() {
            self.tree_followed = active;
        }
        let shown = self.ws.tree.project.filter(|id| self.ws.project(*id).is_some());
        let project =
            if moved { active } else { shown.or(active).or(self.ws.projects.first().map(|p| p.id)) };
        if let Some(project) = project {
            self.show_tree(project);
        }
    }

    /// Shows the files of a project in the column.
    pub(super) fn show_tree(&mut self, project: ProjectId) {
        if self.ws.tree.project == Some(project) && self.ws.tree.unfolded() {
            return;
        }
        let Some(root) = self.ws.project(project).map(|p| p.path.clone()) else { return };
        for dir in self.ws.tree.show(project, &root) {
            self.dirty = true;
            self.files.send(Request::Folder { dir, known: None });
        }
    }

    /// A click on a row of the column.
    pub(super) fn tree_row(&mut self, id: u32) {
        let clicked = self.ws.tree.click(id);
        self.tree_clicked(clicked);
    }

    fn tree_clicked(&mut self, clicked: Option<Clicked>) {
        match clicked {
            Some(Clicked::Read(dir)) => self.files.send(Request::Folder { dir, known: None }),
            Some(Clicked::Link(path)) => self.files.send(Request::Link(path)),
            Some(Clicked::File(path)) => self.show_file(&path),
            Some(Clicked::Closed) | None => {}
        }
    }

    /// Shows a file of the project the column shows, where the changes
    /// are shown.
    pub(super) fn show_file(&mut self, path: &Path) {
        let Some(project) = self.ws.tree.project.and_then(|id| self.ws.project(id)) else { return };
        let Ok(inside) = path.strip_prefix(&project.path) else { return };
        let (project, inside) = (project.id, inside.to_owned());
        let Ok(opened) = self.ws.open_file(project, inside, grammar_of) else {
            return self.ask(Asked::File(path.to_owned()));
        };
        let Some((_, view)) = self.ws.file() else { return };
        // A click on the file that is shown asks whether it changed: the
        // way to try again for one that could not be read. One that was
        // just opened has no state that is known.
        let (known, of) = (view.stamp(), view.edition());
        self.files.send(Request::File { path: path.to_owned(), known, of });
        if opened {
            self.fit();
        }
    }

    /// Asks the folders that are open and the file that is shown whether
    /// they changed. Only what did is read.
    pub(super) fn refresh_files(&mut self) {
        for (dir, known) in self.ws.tree.open() {
            self.files.send(Request::Folder { dir: dir.to_owned(), known });
        }
        if let (Some(path), Some(view)) = (self.ws.file_path(), self.ws.file_mut()) {
            view.occasion();
            self.files.send(Request::File { path, known: view.stamp(), of: view.edition() });
        }
    }

    /// Whether the column or the pane shows something of a project.
    pub(super) fn shows_files_of(&self, project: ProjectId) -> bool {
        (self.ws.tree.unfolded() && self.ws.tree.project == Some(project))
            || self.ws.file().is_some_and(|(_, view)| view.project == project)
    }

    pub(super) fn files_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Folder(dir, listing) => self.dirty |= self.ws.tree.listed(&dir, listing),
            Reply::Link(path, kind) => {
                self.dirty = true;
                let clicked = self.ws.tree.resolved(&path, kind);
                self.tree_clicked(clicked);
            }
            // An answer for an opening of a file that is not shown any more
            // is dropped, also where the same file was opened again since.
            // The change does not count here: the text that asked may have
            // been read anew meanwhile, and what the disk holds now is
            // still the latest of it.
            Reply::File(of, Some(content)) => {
                // Nor is one that was asked for before the text was last
                // saved: it may hold what the file held before that.
                let Some((pane, _)) = self.ws.file().filter(|(_, view)| view.takes(of)) else {
                    return;
                };
                let rows = self.file_room(pane).0;
                // Marks are asked for with a text that was read, not with
                // a file that could not be: git would have nothing to say
                // of it, and asking again would only be answered alike.
                let read = content.is_ok();
                if let Some(view) = self.ws.file_mut() {
                    view.read(content, rows);
                    self.dirty = true;
                }
                if read {
                    self.ask_marks();
                }
            }
            Reply::File(..) => {}
        }
    }

    /// Asks git what changed in the file that is shown, if it is text.
    pub(super) fn ask_marks(&self) {
        // A text that was changed here has no marks: git knows the file.
        let shown = self.ws.file().filter(|(_, view)| view.text().is_some() && !view.changed());
        let Some((_, view)) = shown else { return };
        if let Some(project) = self.ws.project(view.project) {
            self.git.send(git::Request::Marks(project.path.clone(), view.path.clone(), view.edition()));
        }
    }

    /// Git said what changed in the text `of`, and in which state its
    /// file was meanwhile. Whether the marks are taken is for the view
    /// to say.
    pub(super) fn marks_reply(&mut self, of: Edition, stamp: Option<Stamp>, marks: Option<Marks>) {
        let Some(view) = self.ws.file_mut() else { return };
        match view.take_marks(of, stamp, marks) {
            Taken::Marked(changed) => self.dirty |= changed,
            // The file moved on: its text is asked for, and with the
            // text its marks again. Once, see `FileView::take_marks`.
            Taken::Reread => {
                let (known, of) = (view.stamp(), view.edition());
                if let Some(path) = self.ws.file_path() {
                    self.files.send(Request::File { path, known, of });
                }
            }
            Taken::Left => {}
        }
    }

    /// A click on the number of a marked line: the changes open at the
    /// file and at that line, in the place of the file.
    pub(super) fn file_changes(&mut self, of: Edition, line: u32) {
        // The number was drawn for one text. In another the line may be
        // another line, or the file another file.
        let Some((_, view)) = self.ws.file().filter(|(_, view)| view.edition() == of) else { return };
        // The changes name files as text. A name that is none has no marks.
        let (project, Some(file)) = (view.project, view.path.to_str().map(str::to_owned)) else { return };
        if self.ws.show_changes_at(project, file, line).is_err() {
            return self.ask(Asked::Do(Action::FileChanges(of, line), false));
        }
        self.refresh_git(project);
        self.fit();
    }

    /// A click on the row of a file in the changes: the file is shown in
    /// their place.
    pub(super) fn diff_file(&mut self, row: usize, check: u32) {
        let found = self.ws.panes().iter().find_map(|pane| match pane {
            Pane::Diff(view) => Some((view.project, view.file_at(row, check)?.to_owned())),
            _ => None,
        });
        let Some((project, file)) = found else { return };
        let Some(path) = self.ws.project(project).map(|p| p.path.join(&file)) else { return };
        // The changes give way to the file: no file is shown beside them.
        if self.ws.open_file(project, file.into(), grammar_of) == Ok(true) {
            if let Some((_, view)) = self.ws.file() {
                self.files.send(Request::File { path, known: None, of: view.edition() });
            }
            self.fit();
        }
    }

    /// Keeps the colors of the file that is shown coming: gives the
    /// process its text when that is another, asks for the lines that are
    /// seen and a little more, and ends the process when no file is shown
    /// any more. Called between events, so it has to cost nothing when
    /// all is as it was.
    pub(super) fn follow_colors(&mut self) {
        let Some((pane, _)) = self.ws.file() else {
            if std::mem::take(&mut self.coloring) {
                self.colors.send(colors::Request::Close);
            }
            return;
        };
        let rows = self.file_room(pane).0;
        let height = self.preview_height(pane);
        let Some(view) = self.ws.file_mut() else { return };
        // What is shown is no text: the process holds a text nobody
        // looks at. A file that is still being read may be one, and
        // the process keeps what it knows of its language until then.
        if matches!(view.shown(), Shown::Picture(_) | Shown::Other(_) | Shown::Failed(_)) {
            if std::mem::take(&mut self.coloring) {
                self.colors.send(colors::Request::Close);
            }
            return;
        }
        let of = view.edition();
        // The lines that are seen: those of a preview where the file is
        // shown as it is meant, whose code has the colors of its lines.
        let (top, rows) = view.seen(rows, height);
        let Some((doc, colors, _)) = view.coloring() else { return };
        let Some(grammar) = colors.grammar(doc) else { return };
        // Nothing is written down as sent that was not handed over: the
        // thread that takes it may not have started, and the next round
        // sends again.
        if colors.unsent(of) {
            if !self.colors.send(colors::Request::Text { of, grammar, bytes: doc.bytes().to_vec() }) {
                return;
            }
            colors.sent(of);
            self.coloring = true;
        }
        if let Some((from, count)) = colors.wanted(top, rows, doc.lines())
            && self.colors.send(colors::Request::Lines { of, from, count })
        {
            colors.asked(from, count);
        }
    }

    /// Shows the file of Markdown that is shown as it is meant, or as
    /// its text again. Where no such file is shown, nothing.
    pub(super) fn toggle_preview(&mut self) {
        let Some((pane, _)) = self.ws.file() else { return };
        let rows = self.file_room(pane).0;
        if let Some(view) = self.ws.file_mut() {
            view.switch_preview(rows);
        }
    }

    /// How high the room is that the preview of a file has in its pane.
    pub(super) fn preview_height(&self, pane: usize) -> f32 {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let noted = self.ws.file().map_or(0, |(_, view)| file::noted(view));
        preview::room(&self.ui, file::body(&layout), noted).h
    }

    /// Keeps the preview of the file that is shown coming: asks what
    /// its text is made of when that is another, and makes its rows
    /// when the text came or the width, the sizes or the fonts are
    /// others. What is held for a preview goes back when none is shown.
    /// Called between events, so it has to cost nothing when all is as
    /// it was.
    pub(super) fn follow_preview(&mut self) {
        let Some((pane, view)) = self.ws.file() else { return };
        // Asked for while the text is shown, where what stood against a
        // preview may not stand any more: see `Preview::tries`.
        let shows = view.previews();
        if !shows && !view.tries_preview() {
            if let Some(view) = self.ws.file_mut() {
                view.preview.rest();
            }
            return;
        }
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let room = preview::room(&self.ui, file::body(&layout), file::noted(view));
        let width = room.w;
        let sizes = self.ui.meant;
        let Some(view) = self.ws.file_mut() else { return };
        let of = view.edition();
        // Nothing is written down as asked that was not handed over: the
        // next round asks again.
        if view.preview.wants(of)
            && let Some(doc) = view.text()
            && self.colors.send(colors::Request::Outline { of, bytes: doc.bytes().to_vec() })
        {
            view.preview.asked(of);
            self.coloring = true;
        }
        // The rows are made here and not while a frame is built: how
        // wide a sign is, is asked of the renderer, which may load the
        // font it stands in. Its width alone is asked for, not its image.
        if !shows {
            return;
        }
        let Some(gfx) = self.gfx.as_mut() else { return };
        let key = Key { width, sizes, fonts: gfx.renderer.fonts.generation };
        if view.preview.unlaid(&key) {
            let (renderer, begun) = (&mut gfx.renderer, Instant::now());
            view.preview.lay(key, &mut |c, look, px| renderer.advance(c, preview::style(look), px));
            if let Some(timing) = &self.timing {
                let rows = view.preview.made().map_or(0, |(_, laid, _, _)| laid.rows().len());
                timing.laid(rows, width, begun.elapsed());
            }
            self.dirty = true;
        }
        // Rows that were made anew, the switch and a pane that grew
        // know nothing of the room the view stands in.
        self.dirty |= view.preview.settle(room.h);
    }

    /// Lines of the file that is shown were cut into pieces, or its
    /// colors were given up. Nothing of an answer is taken that does not
    /// fit the text that is there now.
    pub(super) fn colors_reply(&mut self, reply: colors::Reply) {
        let Some((pane, _)) = self.ws.file() else { return };
        let rows = self.file_room(pane).0;
        let Some(view) = self.ws.file_mut() else { return };
        let shown = view.edition();
        let why = match reply {
            // What a text is made of, or why that is not said. Each is
            // for the text that was asked for last, and the view takes
            // no other.
            colors::Reply::Outline { of, meant } => {
                self.dirty |= view.preview.take(of, meant);
                return;
            }
            colors::Reply::NoOutline { of, why } => {
                if view.no_preview(of, why, rows) {
                    log(format_args!("no preview of {}: {why:?}", view.path.display()));
                    self.dirty = true;
                }
                return;
            }
            colors::Reply::Pieces { of, from, lines, unknown } if of == shown => {
                let Some((doc, colors, _)) = view.coloring() else { return };
                if colors.take(doc, from, lines, unknown) {
                    self.dirty = true;
                    return;
                }
                // What answers so is not asked again: it ends.
                self.colors.send(colors::Request::Close);
                End::Broken
            }
            // The process does not hold this text: it is given again.
            colors::Reply::Lost { of } if of == shown => return view.colors.lost(),
            // The process was ended over this text. One that was ended
            // over a text before it says nothing of this one, which was
            // given to a new process since.
            colors::Reply::Ended { of, why } if of == shown => why,
            _ => return,
        };
        log(format_args!("the colors of {} are given up: {why:?}", view.path.display()));
        view.colors.end(why);
        self.dirty = true;
    }

    /// The room the picture of the file that is shown is fitted into, in
    /// pixels of the screen.
    fn picture_room(&self, pane: usize) -> (u32, u32) {
        let layout = self.ui.pane(pane, self.ws.panes().len());
        let Some((_, view)) = self.ws.file() else { return (1, 1) };
        let room = file::picture_room(&self.ui, file::body(&layout), view);
        (room.w.floor().max(1.0) as u32, room.h.floor().max(1.0) as u32)
    }

    /// Keeps the picture of the file that is shown coming: asks for its
    /// pixels when the file is another or in another state, asks again a
    /// moment after its room became one it does not fit, and gives up
    /// the texture and the process when no picture is shown any more.
    /// Called between events, so it has to cost nothing when all is as
    /// it was.
    pub(super) fn follow_picture(&mut self, now: Instant) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        let shown =
            self.ws.file().and_then(|(pane, view)| Some((pane, view.picture()?.shown.map(|(_, id)| id))));
        // The texture is of a picture that is not shown any more.
        if gfx.renderer.picture().is_some() && gfx.renderer.picture() != shown.and_then(|(_, id)| id) {
            gfx.renderer.drop_picture();
        }
        let (side, gfx_picture) = (gfx.renderer.max_side, gfx.renderer.picture());
        let Some((pane, _)) = shown else {
            self.again.rest();
            if std::mem::take(&mut self.picturing) {
                self.pictures.send(pictures::Request::Close);
            }
            return;
        };
        let room = self.picture_room(pane);
        let Some(path) = self.ws.file_path() else { return };
        let Some(seen) = self.ws.file_mut().and_then(|view| view.picture_mut()) else { return };
        // The texture of the picture that is shown is gone, with the
        // window it was made for: it is decoded anew.
        if seen.shown.is_some() && gfx_picture.is_none() {
            (seen.shown, seen.stale) = (None, true);
        }
        let base = Wish { format: seen.format, room, side, whole: WHOLE, part: [0; 4] };
        // What is to be shown of the picture that is there, if one is:
        // where it lies and how large. What is asked for follows from
        // it, and from nothing else.
        let (wish, misfits) = match seen.shown {
            Some((decoded, _)) => {
                let frame = seen.look.frame(decoded.picture, (room.0 as f32, room.1 as f32));
                let need = frame.need();
                let wish = picture::ask(decoded.picture, &need, &base);
                // What the wish would give again is no reason to ask.
                let again = seen.misfits(&need) && !picture::is_answer(&decoded, &wish);
                (wish, again)
            }
            None => (base, false),
        };
        let key = picture::key(room, &wish);
        if !seen.stale {
            // A decoding is under way: what it brings is looked at when
            // it is there.
            if seen.asked.is_some() {
                self.again.rest();
                return;
            }
            if !self.again.now(misfits, key, now, PICTURE_DELAY) {
                return;
            }
        }
        // Nothing is written down as asked that was not handed over: the
        // next round asks again.
        self.decodings += 1;
        let asked = pictures::Asked { id: self.decodings, path, wish };
        if self.pictures.send(pictures::Request::Picture(asked)) {
            (seen.asked, seen.stale) = (Some(self.decodings), false);
            self.again.asked(key);
            self.picturing = true;
            return;
        }
        // The thread that has the pictures could not be started: said,
        // and not tried again at every event. The state of the file is
        // forgotten, so that the next occasion reads it and asks anew.
        (seen.stale, seen.why) = (false, Some("It could not be decoded.".to_owned()));
        if seen.shown.is_none()
            && let Some(view) = self.ws.file_mut()
        {
            view.unsettled();
        }
        self.dirty = true;
    }

    /// The answer for a picture lies ready. It is taken only by the
    /// decoding that was asked for last, of the file that is shown.
    pub(super) fn picture_reply(&mut self) {
        let Some(reply) = self.pictures.take() else { return };
        let Some(view) = self.ws.file_mut() else { return };
        let Some(seen) = view.picture_mut().filter(|seen| seen.asked == Some(reply.id)) else { return };
        seen.asked = None;
        if let Some(stamp) = reply.stamp {
            seen.length = stamp.size;
        }
        // A program is writing the file, or was just now: what was made
        // of it may be half a picture.
        let fresh = !reply.steady || reply.stamp.is_some_and(|stamp| stamp.fresh());
        let had = seen.shown.is_some();
        // Whether what was made is not what the file will hold: then it
        // is read again at the next occasion.
        let unsettled = match reply.outcome {
            Outcome::Picture { decoded, pixels } => {
                let partial = decoded.notes.partial() || !reply.steady;
                // The picture that is shown stays while its file is
                // written: half a picture does not take its place.
                if !(partial && fresh && had) {
                    let loaded = self
                        .gfx
                        .as_mut()
                        .is_some_and(|gfx| gfx.renderer.load_picture(reply.id, decoded.size, &pixels));
                    if loaded {
                        (seen.shown, seen.why) = (Some((decoded, reply.id)), None);
                    } else if !had {
                        seen.why = Some("It could not be loaded to be drawn.".into());
                    }
                }
                partial && fresh
            }
            _ if fresh && had => true,
            outcome => {
                seen.shown = None;
                seen.why = Some(match outcome {
                    Outcome::Refused(reason) => reason.reason().to_owned(),
                    Outcome::Unread(why) => why,
                    _ => "It could not be decoded.".to_owned(),
                });
                fresh
            }
        };
        if unsettled {
            view.unsettled();
        }
        self.dirty = true;
    }
}
