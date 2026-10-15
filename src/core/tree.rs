//! The files of a project as a tree: which folders are open, what they
//! hold, and the rows that come of it. Reading is `files`, drawing is
//! `ui::tree`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::ProjectId;
use super::files::{Entry, Kind, Listing};

/// Most entries all open folders hold together, and so most rows.
const MAX_ROWS: usize = 200_000;
/// How deep folders can be opened. A link can lead into the folder it
/// lies in, and each opening of it is one level more.
const MAX_DEPTH: usize = 64;
/// For how many projects it is remembered which folders were open, and
/// how many folders of each.
const MAX_KEPT: usize = 32;
const MAX_KEPT_OPEN: usize = 256;

struct Item {
    entry: Entry,
    /// Stands for the entry in what a click on its row asks for. It stays
    /// with a name through a reading again, so that a click that began
    /// before it still means the same file.
    id: u32,
    /// A folder, or a link to one, whose entries are shown below it.
    open: bool,
    /// A link that was clicked and led to nothing.
    broken: bool,
}

struct Folder {
    path: PathBuf,
    state: State,
}

/// What is known of the entries of a folder.
enum State {
    /// Nothing yet: it is being read.
    Unread,
    Listed {
        items: Vec<Item>,
        /// How many entries it has beyond these.
        more: usize,
        /// When it last changed, to ask whether it did since.
        changed: Option<SystemTime>,
    },
    /// Why it could not be read.
    Failed(String),
}

impl Folder {
    fn unread(path: PathBuf) -> Self {
        Self { path, state: State::Unread }
    }

    /// The entries that were read of it: none before that, and none of
    /// a folder that could not be read.
    fn items(&self) -> &[Item] {
        match &self.state {
            State::Listed { items, .. } => items,
            State::Unread | State::Failed(_) => &[],
        }
    }

    fn items_mut(&mut self) -> &mut [Item] {
        match &mut self.state {
            State::Listed { items, .. } => items,
            State::Unread | State::Failed(_) => &mut [],
        }
    }
}

enum Row {
    Item { folder: usize, item: usize, depth: usize },
    Note { text: String, depth: usize },
}

/// A row as it is drawn.
pub enum Line<'a> {
    Entry {
        id: u32,
        name: &'a str,
        kind: Kind,
        open: bool,
        broken: bool,
        depth: usize,
    },
    /// Something to say about a folder, in the place of entries.
    Note {
        text: &'a str,
        depth: usize,
    },
}

/// What a click on a row leads to.
#[derive(Debug, PartialEq)]
pub enum Clicked {
    /// A folder was opened: its entries are to be read.
    Read(PathBuf),
    /// A folder was closed.
    Closed,
    /// A file: it is to be shown.
    File(PathBuf),
    /// A link: what it points to is to be asked.
    Link(PathBuf),
}

/// Which folders of a project were open, for when the tree shows it again.
struct Kept {
    root: PathBuf,
    open: Vec<PathBuf>,
    scroll: f32,
}

#[derive(Default)]
pub struct Tree {
    /// The project whose files are shown, and its folder.
    pub project: Option<ProjectId>,
    root: PathBuf,
    /// The open folders, the one of the project first and every other
    /// behind the one it lies in. Empty while the tree is folded away.
    folders: Vec<Folder>,
    rows: Vec<Row>,
    /// How far the rows are scrolled, counted in rows.
    scroll: f32,
    /// Where a tree that is being read anew was scrolled to before.
    wanted: Option<f32>,
    /// The link that was clicked and has not said yet where it leads.
    asked: Option<PathBuf>,
    next: u32,
    kept: Vec<Kept>,
}

impl Tree {
    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    pub fn line(&self, row: usize) -> Option<Line<'_>> {
        Some(match self.rows.get(row)? {
            Row::Note { text, depth } => Line::Note { text, depth: *depth },
            Row::Item { folder, item, depth } => {
                let item = self.folders.get(*folder)?.items().get(*item)?;
                Line::Entry {
                    id: item.id,
                    name: &item.entry.shown,
                    kind: item.entry.kind,
                    open: item.open,
                    broken: item.broken,
                    depth: *depth,
                }
            }
        })
    }

    /// The path a row stands for.
    pub fn path(&self, row: usize) -> Option<PathBuf> {
        let Row::Item { folder, item, .. } = self.rows.get(row)? else { return None };
        let folder = self.folders.get(*folder)?;
        Some(folder.path.join(&folder.items().get(*item)?.entry.name))
    }

    /// Whether the tree holds entries: it is open and shows a project.
    pub fn unfolded(&self) -> bool {
        !self.folders.is_empty()
    }

    /// How far the rows are scrolled in a view of `visible` rows.
    pub fn scrolled(&self, visible: f32) -> f32 {
        let lowest = (self.rows.len() as f32 - visible).max(0.0);
        self.wanted.unwrap_or(self.scroll).clamp(0.0, lowest)
    }

    pub fn scroll_by(&mut self, rows: f32, visible: f32) {
        let lowest = (self.rows.len() as f32 - visible).max(0.0);
        self.scroll = (self.scrolled(visible) + rows).clamp(0.0, lowest);
        self.wanted = None;
    }

    fn index(&self, path: &Path) -> Option<usize> {
        self.folders.iter().position(|folder| folder.path == path)
    }

    fn keep(&mut self) {
        if self.folders.is_empty() {
            return;
        }
        let open =
            self.folders.iter().skip(1).take(MAX_KEPT_OPEN).map(|folder| folder.path.clone()).collect();
        let scroll = self.wanted.unwrap_or(self.scroll);
        self.kept.retain(|kept| kept.root != self.root);
        self.kept.push(Kept { root: self.root.clone(), open, scroll });
        let over = self.kept.len().saturating_sub(MAX_KEPT);
        self.kept.drain(..over);
    }

    /// Shows the files of a project. Returns the folders to read: its own
    /// and those that were open when the tree last showed it. None if it
    /// shows them already.
    pub fn show(&mut self, project: ProjectId, root: &Path) -> Vec<PathBuf> {
        if self.project == Some(project) && self.unfolded() {
            return Vec::new();
        }
        self.keep();
        (self.project, self.root) = (Some(project), root.to_owned());
        self.folders = vec![Folder::unread(root.to_owned())];
        (self.scroll, self.wanted, self.asked) = (0.0, None, None);
        if let Some(kept) = self.kept.iter().find(|kept| kept.root == root) {
            self.folders.extend(kept.open.iter().cloned().map(Folder::unread));
            self.wanted = Some(kept.scroll);
        }
        self.rebuild();
        self.folders.iter().map(|folder| folder.path.clone()).collect()
    }

    /// Lets go of the entries. Which folders were open is remembered.
    pub fn fold(&mut self) {
        self.keep();
        self.folders = Vec::new();
        self.rows = Vec::new();
        self.asked = None;
    }

    /// Forgets a project that is gone.
    pub fn forget(&mut self, project: ProjectId, root: &Path) {
        self.kept.retain(|kept| kept.root != root);
        if self.project == Some(project) {
            *self = Self { kept: std::mem::take(&mut self.kept), next: self.next, ..Self::default() };
        }
    }

    /// The open folders with the time each last changed at, to ask them
    /// whether they changed since.
    pub fn open(&self) -> impl Iterator<Item = (&Path, Option<SystemTime>)> {
        self.folders.iter().map(|folder| {
            let changed = match &folder.state {
                State::Listed { changed, .. } => *changed,
                State::Unread | State::Failed(_) => None,
            };
            (folder.path.as_path(), changed)
        })
    }

    fn find(&self, id: u32) -> Option<(usize, usize)> {
        self.rows.iter().find_map(|row| match row {
            Row::Item { folder, item, .. } if self.folders[*folder].items()[*item].id == id => {
                Some((*folder, *item))
            }
            _ => None,
        })
    }

    /// How many folders lie between a path and the folder of the project.
    fn depth(&self, path: &Path) -> usize {
        path.strip_prefix(&self.root).map_or(MAX_DEPTH, |below| below.components().count())
    }

    /// A click on the row of an entry.
    pub fn click(&mut self, id: u32) -> Option<Clicked> {
        let (folder, item) = self.find(id)?;
        let path = self.folders[folder].path.join(&self.folders[folder].items()[item].entry.name);
        let entry = &mut self.folders[folder].items_mut()[item];
        // The rows stay where they are shown, also where that is the
        // place a tree that came back was scrolled to before.
        self.scroll = self.wanted.take().unwrap_or(self.scroll);
        // Only the latest click waits for an answer.
        self.asked = None;
        match entry.entry.kind {
            _ if entry.open => {
                entry.open = false;
                self.folders.retain(|folder| !folder.path.starts_with(&path));
                self.rebuild();
                Some(Clicked::Closed)
            }
            Kind::Folder => self.unfold(folder, item, path),
            Kind::Link => {
                self.asked = Some(path.clone());
                Some(Clicked::Link(path))
            }
            Kind::File | Kind::Other => Some(Clicked::File(path)),
        }
    }

    fn unfold(&mut self, folder: usize, item: usize, path: PathBuf) -> Option<Clicked> {
        if self.depth(&path) > MAX_DEPTH {
            return None;
        }
        self.folders[folder].items_mut()[item].open = true;
        self.folders.push(Folder::unread(path.clone()));
        self.rebuild();
        Some(Clicked::Read(path))
    }

    /// A link was asked what it points to. That it leads nowhere is
    /// taken from any answer. What it leads to is acted on only while the
    /// click that asked still waits: an answer that comes late, from a
    /// drive that took its time, must not open what nobody waits for.
    pub fn resolved(&mut self, link: &Path, kind: Option<Kind>) -> Option<Clicked> {
        let waited = self.asked.as_deref() == Some(link);
        if waited {
            self.asked = None;
        }
        let (parent, name) = (link.parent()?, link.file_name()?);
        let folder = self.index(parent)?;
        let item = self.folders[folder].items().iter().position(|item| item.entry.name == name)?;
        let entry = &mut self.folders[folder].items_mut()[item];
        if entry.entry.kind != Kind::Link {
            return None;
        }
        entry.broken = kind.is_none();
        if !waited {
            return None;
        }
        match kind {
            Some(Kind::Folder) if !entry.open => self.unfold(folder, item, link.to_owned()),
            Some(Kind::Folder) | None => None,
            Some(_) => Some(Clicked::File(link.to_owned())),
        }
    }

    /// The entries of a folder were read, or it is as it was (`None`).
    /// Returns whether that changed anything.
    pub fn listed(&mut self, dir: &Path, listing: Option<Result<Listing, String>>) -> bool {
        let (Some(at), Some(listing)) = (self.index(dir), listing) else { return false };
        // The row at the top stays the row at the top, whatever is
        // sorted in above it.
        let top = self.scroll.floor().max(0.0) as usize;
        let anchor = match self.rows.get(top) {
            Some(Row::Item { folder, item, .. }) if self.wanted.is_none() => {
                Some(self.folders[*folder].items()[*item].id)
            }
            _ => None,
        };
        let listing = listing.and_then(|mut listing| {
            // What no folder of this machine names, and what would make
            // a path lead somewhere else than into the folder.
            listing.entries.retain(|entry| {
                let name = entry.name.as_encoded_bytes();
                !name.is_empty() && name != b"." && name != b".." && !name.contains(&b'/')
            });
            // All open folders together hold only so many entries.
            let held: usize = self
                .folders
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != at)
                .map(|(_, folder)| folder.items().len())
                .sum();
            if held + listing.entries.len() > MAX_ROWS {
                return Err("Too much is open to show this as well. Close a folder.".into());
            }
            Ok(listing)
        });
        // The same entries as before: nothing to build anew and to draw.
        if let (Ok(listing), State::Listed { items, more, changed }) = (&listing, &mut self.folders[at].state)
            && *more == listing.more
            && items.len() == listing.entries.len()
            && items.iter().zip(&listing.entries).all(|(item, entry)| item.entry == *entry)
        {
            *changed = listing.changed;
            return false;
        }
        match listing {
            Ok(listing) => {
                // A set, asked once for every entry: held against a list,
                // a folder of a hundred thousand entries cost the window
                // thread as many looks as folders are open, for each.
                let open: HashSet<&Path> = self.folders.iter().map(|folder| folder.path.as_path()).collect();
                let old: HashMap<&OsStr, &Item> =
                    self.folders[at].items().iter().map(|item| (item.entry.name.as_os_str(), item)).collect();
                let mut next = self.next;
                let items = listing.entries.into_iter().map(|entry| {
                    let was = old.get(entry.name.as_os_str());
                    // A folder that was open before the tree was folded
                    // away is open again, and so is one that stayed.
                    let folds = matches!(entry.kind, Kind::Folder | Kind::Link);
                    let open = folds
                        && (was.is_some_and(|was| was.open)
                            || open.contains(dir.join(&entry.name).as_path()));
                    let id = was.map_or_else(|| (next = next.wrapping_add(1), next).1, |was| was.id);
                    Item {
                        broken: was.is_some_and(|was| was.broken && was.entry.kind == entry.kind),
                        entry,
                        id,
                        open,
                    }
                });
                let items = items.collect();
                self.next = next;
                self.folders[at].state =
                    State::Listed { items, more: listing.more, changed: listing.changed };
            }
            Err(why) => self.folders[at].state = State::Failed(why),
        }
        self.prune();
        self.rebuild();
        if let Some(id) = anchor
            && let Some(row) = self.rows.iter().position(|row| matches!(row, Row::Item { folder, item, .. } if self.folders[*folder].items()[*item].id == id))
        {
            self.scroll = row as f32 + self.scroll.fract();
        }
        true
    }

    /// Lets go of open folders whose entry is gone or closed. A folder
    /// whose own folder has not been read yet stays: nothing is known of it.
    fn prune(&mut self) {
        let mut stays: Vec<bool> = Vec::with_capacity(self.folders.len());
        for (at, folder) in self.folders.iter().enumerate() {
            let parent =
                folder.path.parent().and_then(|parent| self.index(parent)).filter(|parent| *parent < at);
            stays.push(match parent {
                _ if at == 0 => true,
                Some(parent) if stays[parent] => {
                    let parent = &self.folders[parent];
                    let name = folder.path.file_name();
                    matches!(parent.state, State::Unread)
                        || parent
                            .items()
                            .iter()
                            .any(|item| item.open && Some(item.entry.name.as_os_str()) == name)
                }
                _ => false,
            });
        }
        let mut stays = stays.into_iter();
        self.folders.retain(|_| stays.next().unwrap_or(false));
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        if !self.folders.is_empty() {
            self.walk(0, 0, &mut rows);
        }
        self.rows = rows;
    }

    /// The rows of a folder and of what is open in it. Their number is
    /// bounded by what the open folders may hold together, see `listed`,
    /// and by a note or two for each of them.
    fn walk(&self, at: usize, depth: usize, rows: &mut Vec<Row>) {
        let folder = &self.folders[at];
        let note = |text: String| Row::Note { text, depth };
        match &folder.state {
            State::Unread => rows.push(note("Reading…".into())),
            State::Failed(why) => rows.push(note(why.clone())),
            State::Listed { .. } => {}
        }
        for (item, entry) in folder.items().iter().enumerate() {
            rows.push(Row::Item { folder: at, item, depth });
            if entry.open
                && depth < MAX_DEPTH
                && let Some(child) = self.index(&folder.path.join(&entry.entry.name))
            {
                self.walk(child, depth + 1, rows);
            }
        }
        if let State::Listed { more, .. } = &folder.state
            && *more > 0
        {
            // Not the last ones by name: whichever the system named last.
            rows.push(note(format!("{more} more are not shown: too many for one folder.")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry { name: name.into(), shown: name.into(), kind }
    }

    fn listing(names: &[(&str, Kind)]) -> Option<Result<Listing, String>> {
        let entries = names.iter().map(|(name, kind)| entry(name, *kind)).collect();
        Some(Ok(Listing { entries, more: 0, changed: Some(SystemTime::UNIX_EPOCH) }))
    }

    fn names(tree: &Tree) -> Vec<String> {
        (0..tree.rows())
            .map(|row| match tree.line(row).unwrap() {
                Line::Entry { name, depth, open, .. } => {
                    format!("{}{}{name}", "  ".repeat(depth), if open { "v " } else { "" })
                }
                Line::Note { text, depth } => format!("{}({text})", "  ".repeat(depth)),
            })
            .collect()
    }

    fn id(tree: &Tree, name: &str) -> u32 {
        (0..tree.rows())
            .find_map(|row| match tree.line(row).unwrap() {
                Line::Entry { id, name: shown, .. } if shown == name => Some(id),
                _ => None,
            })
            .unwrap()
    }

    fn shown() -> (Tree, PathBuf) {
        let root = PathBuf::from("/p/trail");
        let mut tree = Tree::default();
        assert_eq!(tree.show(ProjectId(1), &root), std::slice::from_ref(&root));
        assert_eq!(names(&tree), ["(Reading…)"]);
        tree.listed(&root, listing(&[("src", Kind::Folder), ("link", Kind::Link), ("a.txt", Kind::File)]));
        (tree, root)
    }

    #[test]
    fn folders_open_and_close_and_files_are_shown() {
        let (mut tree, root) = shown();
        assert_eq!(names(&tree), ["src", "link", "a.txt"]);
        assert_eq!(tree.click(id(&tree, "a.txt")), Some(Clicked::File(root.join("a.txt"))));

        let src = root.join("src");
        assert_eq!(tree.click(id(&tree, "src")), Some(Clicked::Read(src.clone())));
        assert_eq!(names(&tree), ["v src", "  (Reading…)", "link", "a.txt"]);
        tree.listed(&src, listing(&[("ui", Kind::Folder), ("main.rs", Kind::File)]));
        tree.click(id(&tree, "ui"));
        tree.listed(&src.join("ui"), listing(&[("mod.rs", Kind::File)]));
        assert_eq!(names(&tree), ["v src", "  v ui", "    mod.rs", "  main.rs", "link", "a.txt"]);
        assert_eq!(tree.path(2), Some(src.join("ui/mod.rs")));

        // Closing a folder closes what was open inside it.
        assert_eq!(tree.click(id(&tree, "src")), Some(Clicked::Closed));
        assert_eq!(names(&tree), ["src", "link", "a.txt"]);
        tree.click(id(&tree, "src"));
        tree.listed(&src, listing(&[("ui", Kind::Folder), ("main.rs", Kind::File)]));
        assert_eq!(names(&tree), ["v src", "  ui", "  main.rs", "link", "a.txt"]);
        // An answer for a folder that is not open changes nothing.
        assert!(!tree.listed(&src.join("ui"), listing(&[("mod.rs", Kind::File)])));
        // One that could not be read says so, and one that is as it was
        // stays as it is.
        assert!(tree.listed(&src, Some(Err("It may not be read.".into()))));
        assert_eq!(names(&tree), ["v src", "  (It may not be read.)", "link", "a.txt"]);
        assert!(!tree.listed(&src, None));
    }

    #[test]
    fn a_link_is_asked_where_it_leads_when_it_is_clicked() {
        let (mut tree, root) = shown();
        let link = root.join("link");
        assert_eq!(tree.click(id(&tree, "link")), Some(Clicked::Link(link.clone())));
        // To nothing: the row says so.
        assert_eq!(tree.resolved(&link, None), None);
        assert!(matches!(tree.line(1), Some(Line::Entry { broken: true, .. })));
        // To a file: it is shown. To a folder: it opens like one.
        tree.click(id(&tree, "link"));
        assert_eq!(tree.resolved(&link, Some(Kind::File)), Some(Clicked::File(link.clone())));
        tree.click(id(&tree, "link"));
        assert_eq!(tree.resolved(&link, Some(Kind::Folder)), Some(Clicked::Read(link.clone())));
        tree.listed(&link, listing(&[("inner", Kind::File)]));
        assert_eq!(names(&tree), ["src", "v link", "  inner", "a.txt"]);
        assert_eq!(tree.click(id(&tree, "link")), Some(Clicked::Closed));
    }

    #[test]
    fn a_reading_again_keeps_what_was_open_and_the_ids() {
        let (mut tree, root) = shown();
        let src = root.join("src");
        tree.click(id(&tree, "src"));
        tree.listed(&src, listing(&[("main.rs", Kind::File)]));
        let (file, main) = (id(&tree, "a.txt"), id(&tree, "main.rs"));
        // An agent made files, sorted in above: the ids stay with the names.
        tree.listed(
            &root,
            listing(&[
                ("docs", Kind::Folder),
                ("src", Kind::Folder),
                ("Makefile", Kind::File),
                ("a.txt", Kind::File),
            ]),
        );
        assert_eq!(names(&tree), ["docs", "v src", "  main.rs", "Makefile", "a.txt"]);
        assert_eq!((id(&tree, "a.txt"), id(&tree, "main.rs")), (file, main));
        assert_ne!(id(&tree, "docs"), id(&tree, "Makefile"));
        // The folder is gone: what was open inside it goes with it.
        tree.listed(&root, listing(&[("a.txt", Kind::File)]));
        assert_eq!(names(&tree), ["a.txt"]);
        assert_eq!(tree.open().count(), 1);
        // A click that began before on a row that is gone means nothing.
        assert_eq!(tree.click(main), None);
    }

    #[test]
    fn the_row_at_the_top_stays_when_names_are_sorted_in_above_it() {
        let (mut tree, root) = shown();
        tree.scroll_by(2.5, 1.0);
        assert_eq!(tree.scrolled(1.0), 2.0, "no further than the last row");
        tree.listed(
            &root,
            listing(&[
                ("a", Kind::Folder),
                ("b", Kind::Folder),
                ("src", Kind::Folder),
                ("a.txt", Kind::File),
            ]),
        );
        assert_eq!(tree.scrolled(1.0), 3.0);
        assert!(matches!(tree.line(3), Some(Line::Entry { name: "a.txt", .. })));
    }

    #[test]
    fn what_was_open_in_a_project_is_open_again_when_the_tree_comes_back() {
        let (mut tree, root) = shown();
        let src = root.join("src");
        tree.click(id(&tree, "src"));
        tree.listed(&src, listing(&[("ui", Kind::Folder)]));
        tree.click(id(&tree, "ui"));
        tree.listed(&src.join("ui"), listing(&[("mod.rs", Kind::File)]));
        tree.scroll_by(1.0, 2.0);

        // Another project, then back: the folders are read anew, in the
        // order that puts each behind the one it lies in.
        let other = PathBuf::from("/p/other");
        assert_eq!(tree.show(ProjectId(2), &other), std::slice::from_ref(&other));
        assert_eq!(tree.show(ProjectId(1), &root), [root.clone(), src.clone(), src.join("ui")]);
        // The answers may come in any order.
        tree.listed(&src.join("ui"), listing(&[("mod.rs", Kind::File)]));
        tree.listed(&root, listing(&[("src", Kind::Folder), ("a.txt", Kind::File)]));
        tree.listed(&src, listing(&[("ui", Kind::Folder)]));
        assert_eq!(names(&tree), ["v src", "  v ui", "    mod.rs", "a.txt"]);
        assert_eq!(tree.scrolled(2.0), 1.0);

        // Folded away it holds nothing, and comes back the same way.
        tree.fold();
        assert!(!tree.unfolded() && tree.rows() == 0 && tree.open().count() == 0);
        assert_eq!(tree.show(ProjectId(1), &root).len(), 3);
        assert!(tree.show(ProjectId(1), &root).is_empty(), "it shows the project already");
        // A project that is removed is forgotten.
        tree.forget(ProjectId(1), &root);
        assert!(tree.project.is_none() && !tree.unfolded());
        assert_eq!(tree.show(ProjectId(3), &root), [root]);
    }

    #[test]
    fn a_folder_that_could_not_be_read_holds_nothing_that_was_open_in_it() {
        let (mut tree, root) = shown();
        let src = root.join("src");
        tree.click(id(&tree, "src"));
        tree.listed(&src, listing(&[("ui", Kind::Folder)]));
        tree.click(id(&tree, "ui"));
        // Another project, then back: what was open is open again, also
        // while nothing is known yet of the folder it lies in.
        assert_eq!(tree.show(ProjectId(2), Path::new("/p/other")).len(), 1);
        assert_eq!(tree.show(ProjectId(1), &root).len(), 3);
        tree.listed(&src.join("ui"), listing(&[("mod.rs", Kind::File)]));
        assert_eq!(tree.open().count(), 3);
        // The folder of the project cannot be read: what was open in it
        // goes, and an answer for it that comes late brings nothing back.
        assert!(tree.listed(&root, Some(Err("It may not be read.".into()))));
        assert_eq!(names(&tree), ["(It may not be read.)"]);
        assert_eq!(tree.open().count(), 1);
        assert!(!tree.listed(&src, listing(&[("ui", Kind::Folder)])));
        // It is asked again as one of which nothing is known, and read
        // then it shows its entries in the place of what it said.
        assert_eq!(tree.open().next(), Some((root.as_path(), None)));
        assert!(tree.listed(&root, listing(&[("src", Kind::Folder)])));
        assert_eq!(names(&tree), ["src"]);
        assert_eq!(tree.open().next(), Some((root.as_path(), Some(SystemTime::UNIX_EPOCH))));
    }

    #[test]
    fn a_folder_over_the_limit_says_how_much_is_missing() {
        let (mut tree, root) = shown();
        let entries = vec![entry("a", Kind::File)];
        tree.listed(&root, Some(Ok(Listing { entries, more: 7, changed: None })));
        assert_eq!(names(&tree), ["a", "(7 more are not shown: too many for one folder.)"]);
    }

    #[test]
    fn folders_open_only_so_deep() {
        let (mut tree, root) = shown();
        let mut path = root.clone();
        let mut opened = 0;
        // A link into its own folder, opened again and again.
        for _ in 0..MAX_DEPTH + 5 {
            tree.listed(&path, listing(&[("loop", Kind::Folder)]));
            let Some(row) = (0..tree.rows())
                .rev()
                .find(|row| matches!(tree.line(*row), Some(Line::Entry { open: false, .. })))
            else {
                break;
            };
            let Some(Line::Entry { id, .. }) = tree.line(row) else { break };
            if tree.click(id).is_none() {
                break;
            }
            path = path.join("loop");
            opened += 1;
        }
        assert_eq!(opened, MAX_DEPTH);
    }

    #[test]
    fn a_click_keeps_the_place_the_tree_came_back_to() {
        let root = PathBuf::from("/p/trail");
        let mut tree = Tree::default();
        tree.show(ProjectId(1), &root);
        let files: Vec<String> = (0..100).map(|n| format!("file-{n:03}")).collect();
        let files: Vec<(&str, Kind)> = files.iter().map(|name| (name.as_str(), Kind::File)).collect();
        tree.listed(&root, listing(&files));
        tree.scroll_by(40.0, 20.0);
        tree.fold();
        tree.show(ProjectId(1), &root);
        tree.listed(&root, listing(&files));
        assert_eq!(tree.scrolled(20.0), 40.0);
        // The click must not throw the rows back to their top.
        assert!(tree.click(id(&tree, "file-045")).is_some());
        assert_eq!(tree.scrolled(20.0), 40.0);
    }

    #[test]
    fn an_answer_for_a_link_counts_only_while_its_click_waits_for_it() {
        let (mut tree, root) = shown();
        let link = root.join("link");
        // Nobody clicked: an answer opens nothing, for a link or for
        // anything else.
        assert_eq!(tree.resolved(&link, Some(Kind::File)), None);
        assert_eq!(tree.resolved(&root.join("a.txt"), Some(Kind::File)), None);
        assert_eq!(tree.resolved(&root.join("src"), Some(Kind::Folder)), None);
        assert_eq!(names(&tree), ["src", "link", "a.txt"]);
        // Another click came between the click on the link and its answer.
        tree.click(id(&tree, "link"));
        tree.click(id(&tree, "a.txt"));
        assert_eq!(tree.resolved(&link, Some(Kind::File)), None);
        // The tree was folded away and shown again meanwhile.
        tree.click(id(&tree, "link"));
        tree.fold();
        tree.show(ProjectId(1), &root);
        tree.listed(&root, listing(&[("src", Kind::Folder), ("link", Kind::Link), ("a.txt", Kind::File)]));
        assert_eq!(tree.resolved(&link, Some(Kind::Folder)), None);
        // An answer that is late still says that a link leads nowhere.
        assert_eq!(tree.resolved(&link, None), None);
        assert!(matches!(tree.line(1), Some(Line::Entry { broken: true, .. })));
        // And the answer to the click that waits is taken, once.
        tree.click(id(&tree, "link"));
        assert_eq!(tree.resolved(&link, Some(Kind::File)), Some(Clicked::File(link.clone())));
        assert_eq!(tree.resolved(&link, Some(Kind::File)), None);
    }

    #[test]
    fn a_listing_that_says_nothing_new_changes_nothing() {
        let (mut tree, root) = shown();
        let same = [("src", Kind::Folder), ("link", Kind::Link), ("a.txt", Kind::File)];
        let before = id(&tree, "a.txt");
        assert!(!tree.listed(&root, listing(&same)));
        assert_eq!(id(&tree, "a.txt"), before);
        // A link that is a folder now is news, and so is a name less.
        assert!(
            tree.listed(
                &root,
                listing(&[("src", Kind::Folder), ("link", Kind::Folder), ("a.txt", Kind::File)])
            )
        );
        assert!(tree.listed(&root, listing(&[("src", Kind::Folder), ("a.txt", Kind::File)])));
        // After a failure the same entries are news again.
        tree.listed(&root, Some(Err("It may not be read.".into())));
        assert!(tree.listed(&root, listing(&[("src", Kind::Folder), ("a.txt", Kind::File)])));
    }

    #[test]
    fn the_open_folders_hold_only_so_many_entries_together() {
        let root = PathBuf::from("/p/trail");
        let mut tree = Tree::default();
        tree.show(ProjectId(1), &root);
        tree.listed(&root, listing(&[("a", Kind::Folder), ("b", Kind::Folder)]));
        tree.click(id(&tree, "a"));
        tree.click(id(&tree, "b"));
        let many = |count: usize| {
            let entries = (0..count).map(|n| entry(&format!("f{n}"), Kind::File)).collect();
            Some(Ok(Listing { entries, more: 0, changed: None }))
        };
        // With the two of the project this is as much as is held.
        tree.listed(&root.join("a"), many(MAX_ROWS - 2));
        assert_eq!(tree.rows(), MAX_ROWS + 1, "and the row that says the other is being read");
        // One more folder is over it: it says so and holds nothing.
        tree.listed(&root.join("b"), many(1));
        assert_eq!(tree.rows(), MAX_ROWS + 1);
        assert!(matches!(tree.line(MAX_ROWS), Some(Line::Note { depth: 1, .. })));
        // With the large one closed there is room for it.
        tree.click(id(&tree, "a"));
        tree.listed(&root.join("b"), many(1));
        assert_eq!(names(&tree), ["a", "v b", "  f0"]);
    }

    #[test]
    fn names_that_no_folder_gives_are_left_out() {
        let (mut tree, root) = shown();
        tree.listed(
            &root,
            listing(&[
                ("", Kind::Folder),
                (".", Kind::Folder),
                ("..", Kind::Folder),
                ("a/b", Kind::File),
                ("ok", Kind::File),
            ]),
        );
        assert_eq!(names(&tree), ["ok"]);
    }
}
