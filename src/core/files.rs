//! Reading folders and files for the tree and for the pane that shows a
//! file. Everything the two ask of the disk goes through `Requests`, so
//! that a project on another machine can be asked the same one day.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

use super::open_unblocking;
use super::picture::Format;
use super::text::Document;

/// How many reads run side by side. A drive that does not answer holds
/// these and no more.
const WORKERS: usize = 2;
/// Most entries of one folder that are listed. What lies over it is not a
/// tail but whatever the filesystem named last.
pub const MAX_ENTRIES: usize = 100_000;
/// Most bytes the names of one folder may add up to.
const MAX_NAMES: usize = 8 << 20;
/// Most bytes of a file that are read: what the clipboard takes as well.
pub const MAX_FILE: usize = 8 << 20;
/// How many bytes of a file are read before it is known whether it is a
/// picture that is shown as one: such a file is read no further here.
const FIRST: usize = 8 << 10;
/// How often a file is read again because it changed while it was read.
const TRIES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Folder,
    File,
    /// A link, to whatever. It is resolved when it is clicked.
    Link,
    /// Neither, or the system did not say.
    Other,
}

/// One entry of a folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The name as the system has it, which need not be text.
    pub name: OsString,
    /// The name to show.
    pub shown: String,
    pub kind: Kind,
}

/// What a folder holds, folders first, each kind sorted by name.
#[derive(Debug, Default, PartialEq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// How many entries were left out, over the limits.
    pub more: usize,
    /// When the folder last changed, as it said before it was read.
    pub changed: Option<SystemTime>,
}

/// What tells one state of a file from another without reading it.
#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    device: u64,
    inode: u64,
    pub size: u64,
    modified: (i64, i64),
    status: (i64, i64),
    /// Who may do what with the file.
    mode: u32,
    /// Where the name of the file led when it was read, see `led`:
    /// nothing where it was only looked at. No part of what tells two
    /// states apart: the same file under the same name is in one state
    /// whoever asks.
    led: u64,
}

impl PartialEq for Stamp {
    fn eq(&self, other: &Self) -> bool {
        (self.device, self.inode, self.size, self.modified, self.status, self.mode)
            == (other.device, other.inode, other.size, other.modified, other.status, other.mode)
    }
}

/// A number for the file a path leads to, through every link on the way:
/// the same for as long as the name leads to the same place, and another
/// once a link was put under the name or turned. Nothing, 0, where the
/// path leads nowhere. For a worker: it asks the disk.
pub fn led(path: &Path) -> u64 {
    std::fs::canonicalize(path).map_or(0, |whole| digest(whole.as_os_str().as_bytes()).max(1))
}

impl Stamp {
    /// Who may do what with the file, as its permissions say.
    pub fn mode(&self) -> u32 {
        self.mode & 0o7777
    }

    /// Where the name led when the file was read, 0 if that is not known.
    pub fn led(&self) -> u64 {
        self.led
    }

    /// The same state, read under a name that led there.
    #[cfg(test)]
    pub fn leading(self, led: u64) -> Self {
        Self { led, ..self }
    }

    /// Whether the file was written just now: a program may be writing
    /// it still, and what it holds may be half of what it will hold.
    pub fn fresh(&self) -> bool {
        let written = std::time::Duration::new(
            self.modified.0.max(0) as u64,
            self.modified.1.clamp(0, 999_999_999) as u32,
        );
        SystemTime::UNIX_EPOCH.checked_add(written).is_some_and(fresh)
    }
}

/// Which text something was asked of: the opening of a file, a number
/// that is never used twice, and the change of its text, a number that
/// only grows. A path and a state of a file do not say that: open one
/// file, then another, then the first again, and an answer for the first
/// opening fits the second. Every answer about a file carries this, and
/// so does a click that is acted on later.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Edition {
    pub opening: u64,
    pub change: u64,
}

/// A file as it was read.
pub struct Content {
    pub stamp: Stamp,
    /// What tells its bytes from others, see `digest`: of all of it, or
    /// nothing for a file that is longer than is read.
    pub digest: Option<u64>,
    pub body: Body,
}

/// A number for the bytes of a file, to know later whether a file holds
/// the same. The time and the size of a file do not say that: a change of
/// the same length within one step of the clock leaves both, and a time
/// can be set back. Good for one run of codn, and against chance, not
/// against someone who looks for two files with one number.
pub fn digest(bytes: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    hasher.write_usize(bytes.len());
    hasher.finish()
}

pub enum Body {
    Text(Document),
    /// A picture that is shown as one. Nothing of it was read beyond its
    /// first bytes: it is decoded in a process of its own.
    Picture(Format),
    /// Not text. The name of its format, where the first bytes tell.
    Binary(Option<&'static str>),
}

pub enum Request {
    /// The entries of a folder, unless it changed last at `known`.
    Folder { dir: PathBuf, known: Option<SystemTime> },
    /// A file, unless it is still as `known` says, for the text `of`.
    File { path: PathBuf, known: Option<Stamp>, of: Edition },
    /// What a link points to.
    Link(PathBuf),
}

impl Request {
    fn path(&self) -> &Path {
        match self {
            Self::Folder { dir, .. } => dir,
            Self::File { path, .. } | Self::Link(path) => path,
        }
    }

    /// Whether the answer does not depend on what is known already.
    fn unconditional(&self) -> bool {
        matches!(self, Self::Folder { known: None, .. } | Self::File { known: None, .. } | Self::Link(_))
    }

    /// Whether two requests ask for the same thing. What is known of it
    /// does not tell them apart.
    fn same(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other) && self.path() == other.path()
    }
}

/// The answers. `None` where a folder or a file is as it was known. One
/// about a file names the text it was asked of, not the path.
pub enum Reply {
    Folder(PathBuf, Option<Result<Listing, String>>),
    File(Edition, Option<Result<Content, String>>),
    Link(PathBuf, Option<Kind>),
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<Request>,
    /// Being answered right now. The same request waits until that is
    /// done, so a folder that takes long occupies one worker and no more.
    running: Vec<Request>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    started: AtomicBool,
    reply: Box<dyn Fn(Reply) -> bool + Send + Sync>,
}

/// Where requests go, after the model of `git::Requests`: each distinct
/// one waits once and never runs twice at a time. The workers start with
/// the first request, so that a codn in which no tree was opened has
/// none, and end when this is dropped.
pub struct Requests(Arc<Shared>);

impl Requests {
    /// `reply` is called on a worker thread and returns whether anyone is
    /// still listening.
    pub fn new(reply: impl Fn(Reply) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(Shared {
            queue: Mutex::default(),
            ready: Condvar::new(),
            started: AtomicBool::new(false),
            reply: Box::new(reply),
        }))
    }

    pub fn send(&self, request: Request) {
        if !self.0.started.swap(true, Ordering::AcqRel) {
            let spawn = |_| {
                let shared = self.0.clone();
                std::thread::Builder::new().name("codn files".into()).spawn(move || work(&shared)).is_ok()
            };
            // With no worker at all the next request tries again, or
            // everything asked for would stand as "Reading…" for good.
            if (0..WORKERS).filter(|n| spawn(*n)).count() == 0 {
                self.0.started.store(false, Ordering::Release);
            }
        }
        let mut queue = self.0.queue.lock().unwrap();
        // Of two requests for the same thing one waits: the one that reads
        // whatever is known, or else the later, which knows more.
        if let Some(at) = queue.waiting.iter().position(|waiting| waiting.same(&request)) {
            if queue.waiting[at].unconditional() {
                // It is answered to who asked last: the answer to an
                // opening that is gone would be dropped, and the file
                // opened after it would stand as "Reading…" for good.
                if let (Request::File { of, .. }, Request::File { of: later, .. }) =
                    (&mut queue.waiting[at], &request)
                {
                    *of = *later;
                }
                return;
            }
            queue.waiting.remove(at);
        }
        match request {
            // A file is what someone is looking at.
            Request::File { .. } | Request::Link(_) => queue.waiting.push_front(request),
            Request::Folder { .. } => queue.waiting.push_back(request),
        }
        drop(queue);
        self.0.ready.notify_one();
    }
}

impl Drop for Requests {
    fn drop(&mut self) {
        self.0.queue.lock().unwrap().closed = true;
        self.0.ready.notify_all();
    }
}

fn work(shared: &Shared) {
    loop {
        let request = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if queue.closed {
                    return;
                }
                let free = queue.waiting.iter().position(|w| !queue.running.iter().any(|r| r.same(w)));
                if let Some(request) = free.and_then(|at| queue.waiting.remove(at)) {
                    let running = match &request {
                        Request::Folder { dir, .. } => Request::Folder { dir: dir.clone(), known: None },
                        Request::File { path, of, .. } => {
                            Request::File { path: path.clone(), known: None, of: *of }
                        }
                        Request::Link(path) => Request::Link(path.clone()),
                    };
                    queue.running.push(running);
                    break request;
                }
                queue = shared.ready.wait(queue).unwrap();
            }
        };
        let answer = match &request {
            Request::Folder { dir, known } => Reply::Folder(dir.clone(), list(dir, *known)),
            Request::File { path, known, of } => Reply::File(*of, read(path, *known)),
            Request::Link(path) => Reply::Link(path.clone(), resolve(path)),
        };
        let listening = (shared.reply)(answer);
        let mut queue = shared.queue.lock().unwrap();
        queue.running.retain(|running| !running.same(&request));
        queue.closed |= !listening;
        drop(queue);
        // The same request may have been waiting for this one to finish.
        shared.ready.notify_all();
    }
}

/// A reason in a few words, without the number of the error.
pub(crate) fn reason(error: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match error.kind() {
        NotFound => "It is not there any more.".into(),
        PermissionDenied => "It may not be read.".into(),
        _ => {
            let text = error.to_string();
            let text = text.split(" (os error").next().unwrap_or_default();
            format!("It cannot be read: {text}.")
        }
    }
}

/// When a change is too fresh to trust the time of it: the clock of a
/// filesystem counts in steps, and a second change within one step leaves
/// the time as it was.
fn fresh(changed: SystemTime) -> bool {
    changed.elapsed().is_ok_and(|age| age.as_secs() < 2)
}

/// Reads a folder. `None` if it changed last at `known`, and that is not
/// just now.
fn list(dir: &Path, known: Option<SystemTime>) -> Option<Result<Listing, String>> {
    let changed = std::fs::metadata(dir).and_then(|meta| meta.modified()).ok();
    if let (Some(changed), Some(known)) = (changed, known)
        && changed == known
        && !fresh(changed)
    {
        return None;
    }
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) => return Some(Err(reason(&error))),
    };
    let (mut entries, mut more, mut bytes) = (Vec::new(), 0, 0);
    // An entry that cannot be read has no name to list it by.
    for entry in read.flatten() {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        bytes += name.len();
        if entries.len() >= MAX_ENTRIES || bytes > MAX_NAMES {
            more += 1;
            continue;
        }
        let kind = match entry.file_type() {
            Ok(kind) if kind.is_dir() => Kind::Folder,
            Ok(kind) if kind.is_symlink() => Kind::Link,
            Ok(kind) if kind.is_file() => Kind::File,
            _ => Kind::Other,
        };
        entries.push(Entry { shown: shown(&name), name, kind });
    }
    // The key is made once for an entry, not at every comparison. Names
    // that differ in case alone are told apart by their bytes.
    entries.sort_by_cached_key(|entry| {
        (entry.kind != Kind::Folder, entry.shown.to_lowercase(), entry.name.as_bytes().to_vec())
    });
    Some(Ok(Listing { entries, more, changed }))
}

/// A name as it is shown: what is no text, what would move the pen and
/// what would turn the direction of writing become replacement characters.
pub fn shown(name: &std::ffi::OsStr) -> String {
    let turns =
        |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    name.to_string_lossy().chars().map(|c| if c.is_control() || turns(c) { '\u{fffd}' } else { c }).collect()
}

/// What a link points to, followed to its end. Nothing for a link to
/// nothing, or in a circle.
fn resolve(path: &Path) -> Option<Kind> {
    let meta = std::fs::metadata(path).ok()?;
    Some(if meta.is_dir() {
        Kind::Folder
    } else if meta.is_file() {
        Kind::File
    } else {
        Kind::Other
    })
}

pub(crate) fn stamp(meta: &std::fs::Metadata) -> Stamp {
    Stamp {
        device: meta.dev(),
        inode: meta.ino(),
        size: meta.size(),
        modified: (meta.mtime(), meta.mtime_nsec()),
        status: (meta.ctime(), meta.ctime_nsec()),
        mode: meta.mode(),
        led: 0,
    }
}

/// The state a file is in, without reading it. For a worker: it asks
/// the disk.
pub fn stamp_of(path: &Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().filter(|meta| meta.is_file()).map(|meta| stamp(&meta))
}

/// Reads a file, up to `MAX_FILE`. `None` if it is as `known` says.
fn read(path: &Path, known: Option<Stamp>) -> Option<Result<Content, String>> {
    let mut last = None;
    for _ in 0..TRIES {
        let mut file = match open_unblocking(path) {
            Ok(file) => file,
            Err(error) => return Some(Err(reason(&error))),
        };
        let before = match file.metadata() {
            Ok(meta) if meta.is_file() => stamp(&meta),
            Ok(meta) if meta.is_dir() => return Some(Err("It is a folder.".into())),
            Ok(_) => return Some(Err("It is not a regular file.".into())),
            Err(error) => return Some(Err(reason(&error))),
        };
        if known == Some(before) {
            return None;
        }
        // One byte more than is shown, to know that there is more: a file
        // can be longer than it said when it was asked.
        let mut bytes = Vec::new();
        if bytes.try_reserve((before.size as usize).min(MAX_FILE) + 1).is_err() {
            return Some(Err("There is not enough memory to read it.".into()));
        }
        // Its first bytes say whether it is a picture, and one is read
        // no further: megabytes of it would be read to be told from text.
        if let Err(error) = (&mut file).take(FIRST as u64).read_to_end(&mut bytes) {
            return Some(Err(reason(&error)));
        }
        if let Some(format) = Format::of(&bytes) {
            let stamp = Stamp { led: led(path), ..before };
            return Some(Ok(Content { stamp, digest: None, body: Body::Picture(format) }));
        }
        let rest = (MAX_FILE + 1).saturating_sub(bytes.len()) as u64;
        if let Err(error) = (&mut file).take(rest).read_to_end(&mut bytes) {
            return Some(Err(reason(&error)));
        }
        let after = file.metadata().map(|meta| stamp(&meta)).ok();
        let whole = after == Some(before);
        last = Some((bytes, before));
        // A program was writing it meanwhile: what was read may be the
        // half of one state and the half of another.
        if whole {
            break;
        }
    }
    let (mut bytes, stamp) = last?;
    // Where the name led: asked behind the reading, of the name as it is
    // then. A save is held against it.
    let stamp = Stamp { led: led(path), ..stamp };
    let cut = bytes.len() > MAX_FILE;
    bytes.truncate(MAX_FILE);
    let digest = (!cut).then(|| digest(&bytes));
    let body = match Document::new(bytes, cut) {
        Ok(text) => Body::Text(text),
        Err(bytes) => Body::Binary(format(&bytes)),
    };
    Some(Ok(Content { stamp, digest, body }))
}

/// The name of a format that is told by its first bytes.
fn format(bytes: &[u8]) -> Option<&'static str> {
    let begins = |magic: &[u8]| bytes.starts_with(magic);
    Some(match () {
        () if begins(b"GIF87a") || begins(b"GIF89a") => "GIF picture",
        () if begins(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => "WebP picture",
        () if bytes.get(4..12) == Some(b"ftypavif") => "AVIF picture",
        () if begins(b"%PDF-") => "PDF document",
        () if begins(b"PK\x03\x04") => "Zip archive",
        () if begins(b"\x1f\x8b") => "Gzip archive",
        () if begins(b"\x7fELF") => "Program",
        () if begins(b"SQLite format 3\0") => "SQLite database",
        () if begins(b"wOF2") || begins(b"wOFF") || begins(b"OTTO") => "Font",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codn-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_lists_everything_but_git_folders_first() {
        let dir = folder("list");
        for name in ["b.txt", "A.txt", ".env", "a.txt"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        for name in [".git", "src", "Docs", "node_modules"] {
            std::fs::create_dir(dir.join(name)).unwrap();
        }
        std::os::unix::fs::symlink("nowhere", dir.join("link")).unwrap();
        // A name that is no text is listed, with a character in its place.
        let odd = OsString::from_vec(b"odd\xff.txt".to_vec());
        std::fs::write(dir.join(&odd), "x").unwrap();

        let listing = list(&dir, None).unwrap().unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.shown.as_str()).collect();
        assert_eq!(
            names,
            ["Docs", "node_modules", "src", ".env", "A.txt", "a.txt", "b.txt", "link", "odd\u{fffd}.txt"]
        );
        assert_eq!(listing.entries[0].kind, Kind::Folder);
        assert_eq!(listing.entries[7].kind, Kind::Link);
        assert_eq!(listing.entries[8].name, odd);
        assert_eq!(listing.more, 0);

        // A link to nothing resolves to nothing, one to a folder to that.
        assert_eq!(resolve(&dir.join("link")), None);
        std::os::unix::fs::symlink("src", dir.join("to-src")).unwrap();
        assert_eq!(resolve(&dir.join("to-src")), Some(Kind::Folder));

        // A folder that is gone is an error, not an empty folder.
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(list(&dir, None).unwrap().is_err());
    }

    #[test]
    fn a_folder_that_did_not_change_is_not_read_again() {
        let dir = folder("known");
        let first = list(&dir, None).unwrap().unwrap();
        let changed = first.changed.unwrap();
        // Just now is too fresh to be trusted.
        assert!(list(&dir, Some(changed)).is_some());
        let long_ago = SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::open(&dir).unwrap().set_modified(long_ago).unwrap();
        assert!(list(&dir, Some(long_ago)).is_none());
        assert!(list(&dir, Some(changed)).is_some(), "another time than the known one");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_cannot_move_the_pen_or_turn_the_writing() {
        assert_eq!(shown(std::ffi::OsStr::new("a\nb\u{202e}c\u{1b}[31m")), "a\u{fffd}b\u{fffd}c\u{fffd}[31m");
    }

    #[test]
    fn a_file_is_read_up_to_the_limit_and_only_when_it_changed() {
        let dir = folder("read");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let content = read(&path, None).unwrap().unwrap();
        let Body::Text(text) = &content.body else { panic!("text") };
        assert_eq!((text.lines(), text.line(1), text.cut()), (3, &b"two"[..], false));
        assert!(read(&path, Some(content.stamp)).is_none());

        // The same size, written later: the stamp tells.
        std::fs::write(&path, "ONE\nTWO\n").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert!(read(&path, Some(content.stamp)).is_some());

        let long = dir.join("long");
        std::fs::write(&long, vec![b'x'; MAX_FILE + 10]).unwrap();
        let Body::Text(text) = read(&long, None).unwrap().unwrap().body else { panic!("text") };
        assert!(text.cut() && text.line(0).len() == MAX_FILE);

        // A picture is told by its first bytes and read no further: it
        // has no digest, which is of all the bytes of a file.
        let picture = [&b"\x89PNG\r\n\x1a\n"[..], &vec![0; MAX_FILE + 10]].concat();
        std::fs::write(dir.join("p.png"), &picture).unwrap();
        let content = read(&dir.join("p.png"), None).unwrap().unwrap();
        assert!(matches!(content.body, Body::Picture(Format::Png)));
        assert_eq!((content.digest, content.stamp.size), (None, picture.len() as u64));
        assert!(content.stamp.fresh(), "written just now");
        std::fs::write(dir.join("p.jpg"), b"\xff\xd8\xff\xe0\0\0").unwrap();
        assert!(matches!(
            read(&dir.join("p.jpg"), None),
            Some(Ok(Content { body: Body::Picture(Format::Jpeg), .. }))
        ));
        // One that is not shown as a picture has the name of its format.
        std::fs::write(dir.join("p.gif"), b"GIF89a\0\0").unwrap();
        assert!(matches!(
            read(&dir.join("p.gif"), None),
            Some(Ok(Content { body: Body::Binary(Some("GIF picture")), .. }))
        ));
        assert!(read(&dir, None).unwrap().is_err(), "a folder is no file");
        assert!(read(&dir.join("gone"), None).unwrap().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_same_request_waits_once_and_the_later_one_counts() {
        let requests = Requests::new(|_| true);
        // No workers: only the queue is looked at.
        requests.0.started.store(true, Ordering::Release);
        let dir = PathBuf::from("/nowhere");
        let now = SystemTime::now();
        let earlier = now - std::time::Duration::from_secs(9);
        requests.send(Request::Folder { dir: dir.clone(), known: Some(earlier) });
        requests.send(Request::Folder { dir: dir.clone(), known: Some(now) });
        let first = Edition { opening: 1, change: 0 };
        requests.send(Request::File { path: dir.clone(), known: None, of: first });
        {
            let queue = requests.0.queue.lock().unwrap();
            assert_eq!(queue.waiting.len(), 2);
            assert!(matches!(&queue.waiting[0], Request::File { .. }), "a file goes first");
            assert!(matches!(&queue.waiting[1], Request::Folder { known: Some(known), .. } if *known == now));
        }
        // One that reads whatever is known is not replaced by one that may not.
        requests.send(Request::Folder { dir: dir.clone(), known: None });
        requests.send(Request::Folder { dir: dir.clone(), known: Some(now) });
        {
            let queue = requests.0.queue.lock().unwrap();
            assert!(matches!(&queue.waiting[1], Request::Folder { known: None, .. }));
            assert_eq!(queue.waiting.len(), 2);
        }
        // The file was left and opened again before it was read: the one
        // reading that waits is for the opening that is there to take it.
        let second = Edition { opening: 3, change: 0 };
        requests.send(Request::File { path: dir.clone(), known: None, of: second });
        let queue = requests.0.queue.lock().unwrap();
        assert!(matches!(&queue.waiting[0], Request::File { known: None, of, .. } if *of == second));
        assert_eq!(queue.waiting.len(), 2);
    }

    #[test]
    fn the_workers_start_with_the_first_request_answer_and_end() {
        let dir = folder("live");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let requests = Requests::new(move |reply| tx.lock().unwrap().send(reply).is_ok());
        assert!(!requests.0.started.load(Ordering::Acquire));
        requests.send(Request::Folder { dir: dir.clone(), known: None });
        let of = Edition { opening: 7, change: 2 };
        requests.send(Request::File { path: dir.join("a.txt"), known: None, of });
        requests.send(Request::Link(dir.join("nowhere")));
        let wait = std::time::Duration::from_secs(5);
        let (mut folders, mut files, mut links) = (0, 0, 0);
        for _ in 0..3 {
            match rx.recv_timeout(wait).unwrap() {
                Reply::Folder(path, Some(Ok(listing))) => {
                    assert_eq!((path, listing.entries.len()), (dir.clone(), 1));
                    folders += 1;
                }
                Reply::File(asked, Some(Ok(Content { body: Body::Text(text), .. }))) => {
                    assert_eq!((asked, text.line(0)), (of, &b"one"[..]));
                    files += 1;
                }
                Reply::Link(_, None) => links += 1,
                _ => panic!("another answer"),
            }
        }
        assert_eq!((folders, files, links), (1, 1, 1));
        // Nothing runs and nothing waits when all is answered.
        let deadline = std::time::Instant::now() + wait;
        while !requests.0.queue.lock().unwrap().running.is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(requests.0.queue.lock().unwrap().waiting.is_empty());
        // Dropped, the workers let go of what they share.
        let shared = Arc::downgrade(&requests.0);
        drop(requests);
        while shared.strong_count() > 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
