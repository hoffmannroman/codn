//! Writing the text of a file back to it, on a thread of its own: a disk
//! that hangs must not hold the window.
//!
//! A file is written beside itself and renamed onto itself where that
//! leaves it the file it was: one name, the user's own, the group a new
//! file would get, nothing attached to it, in a folder that can be written
//! to. Else it is written in place, which a crash in the middle turns into
//! half a file, and only after the user was told so. Before anything is
//! written the file is read once more: one that another program wrote to
//! meanwhile is not written over.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use super::files::{MAX_FILE, Stamp, digest, led, stamp};

/// A text to write.
pub struct Job {
    pub path: PathBuf,
    /// All of the file as it is to be.
    pub bytes: Vec<u8>,
    /// The digest of the bytes the text was read from. A file that holds
    /// others was written by someone else, and is left alone.
    pub known: Option<u64>,
    /// Write over whatever is there, and make the file anew if it is
    /// gone: the user saw that it is another, and said so.
    pub force: bool,
    /// The user was told that this file is written in place.
    pub in_place: bool,
    /// Where the name led when the text was read, see `files::led`, and
    /// 0 where that is not known. A name that leads elsewhere by now has
    /// a link under it that was not there, and nothing is written
    /// through it, whatever the user said: the line he answered spoke of
    /// another text on the disk, not of another file.
    pub led: u64,
    /// The permissions the file had when it was read, for one that went
    /// and is made anew.
    pub mode: Option<u32>,
}

/// What came of it.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The file holds the text. Its state now, where the file said it,
    /// and whether all of it reached the disk for sure: a flush that
    /// fails after the file is in place is neither "all is well" nor "as
    /// it was".
    Saved { stamp: Option<Stamp>, digest: u64, flushed: bool },
    /// Nothing was written: the file holds another text than was read.
    Other,
    /// Nothing was written: the file is not there any more.
    Gone,
    /// Nothing was written: this file is written in place, and the user
    /// was not told yet.
    InPlace,
    /// Nothing was written, and why.
    Failed(String),
    /// Writing in place failed half way: the file may be damaged, and
    /// the text in the pane is the only one.
    Damaged(String),
}

/// Why something could not be written, in a few words.
fn why(error: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match error.raw_os_error() {
        Some(libc::ENOSPC | libc::EDQUOT | libc::EFBIG) => {
            return "There is no room for it on the disk.".into();
        }
        Some(libc::EROFS) => return "It lies where nothing can be written.".into(),
        _ => {}
    }
    match error.kind() {
        NotFound => "It is not there any more.".into(),
        PermissionDenied => "It may not be written.".into(),
        _ => {
            let text = error.to_string();
            let text = text.split(" (os error").next().unwrap_or_default();
            format!("It cannot be written: {text}.")
        }
    }
}

/// Whether an error says that the disk has no room.
fn full(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT | libc::EFBIG))
}

/// Whether the user may do `mode` to what a path names.
fn may(path: &Path, mode: libc::c_int) -> bool {
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: the path is a string that ends with a zero byte and lives
    // through the call.
    unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS) == 0 }
}

/// Whether something is attached to a file that a new file would not
/// have: attributes that were set on it. The label of SELinux is not
/// counted, a new file in the same folder gets its own.
fn attached(path: &Path) -> bool {
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return true };
    let mut names = vec![0u8; 4096];
    // SAFETY: the path ends with a zero byte, and the list is as long as
    // the call is told.
    let length = unsafe { libc::listxattr(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    match usize::try_from(length) {
        Ok(length) => names[..length]
            .split(|byte| *byte == 0)
            .any(|name| !name.is_empty() && name != b"security.selinux"),
        // A list too long for the room is a list of something. A
        // filesystem that has no attributes has none.
        Err(_) => std::io::Error::last_os_error().raw_os_error() == Some(libc::ERANGE),
    }
}

/// The digest of what an open file holds, read from its start, as far as
/// a file is read at all.
fn holds(file: &mut File) -> std::io::Result<u64> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
    Ok(digest(&bytes))
}

/// The state of a file that was just written, if it can be named as the
/// state of the bytes that were written: the file is read back between
/// two looks at its state. Another program can write to it the moment it
/// is in place, and its state taken then would be kept as that of the
/// own text: the next look at the file would find nothing new in what
/// someone else wrote. Without a state, the file is read at that look.
fn confirmed(file: &mut File, bytes: &[u8]) -> Option<Stamp> {
    let before = stamp(&file.metadata().ok()?);
    let held = holds(file).ok()?;
    let after = stamp(&file.metadata().ok()?);
    (before == after && held == digest(bytes)).then_some(before)
}

/// Whether a file can be replaced by a new one of the same name without
/// becoming another file to anyone: see the top of this file.
fn replaceable(target: &Path, dir: &Path, meta: &Metadata) -> bool {
    // SAFETY: both only read the ids of the process.
    let (user, group) = unsafe { (libc::geteuid(), libc::getegid()) };
    // A folder that hands its group on gives it to a new file.
    let given = match std::fs::metadata(dir) {
        Ok(dir) if dir.mode() & libc::S_ISGID != 0 => dir.gid(),
        Ok(_) => group,
        Err(_) => return false,
    };
    meta.nlink() == 1
        && meta.uid() == user
        && meta.gid() == given
        && !attached(target)
        && may(dir, libc::W_OK | libc::X_OK)
}

/// Writes the text into a new file beside the old one and renames it
/// onto it. An error says that nothing was put in place.
fn beside(target: &Path, dir: &Path, meta: &Metadata, bytes: &[u8]) -> std::io::Result<Outcome> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(".codn-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let tmp = dir.join(name);
    // For the user alone until it has the permissions of the file: it
    // may be a file of secrets.
    let mut file = OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&tmp)?;
    let put = (|| {
        file.write_all(bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(meta.mode() & 0o7777))?;
        file.sync_all()?;
        std::fs::rename(&tmp, target)
    })();
    if let Err(error) = put {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    // The file is in place. Whether its name is on the disk for sure is
    // another matter, and no reason to call the save failed.
    let flushed = File::open(dir).and_then(|dir| dir.sync_all()).is_ok();
    Ok(Outcome::Saved { stamp: confirmed(&mut file, bytes), digest: digest(bytes), flushed })
}

/// Writes the text into the file itself: opened without emptying it,
/// checked once more as the open file, the room asked for where the
/// filesystem gives it, written from the start, cut to the new length.
fn in_place(target: &Path, seen: (u64, u64), job: &Job) -> Outcome {
    let flags = libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY;
    let mut file = match OpenOptions::new().read(true).write(true).custom_flags(flags).open(target) {
        Ok(file) => file,
        Err(error) => return Outcome::Failed(why(&error)),
    };
    let meta = match file.metadata() {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Outcome::Failed("It is not a regular file.".into()),
        Err(error) => return Outcome::Failed(why(&error)),
    };
    // Another file under the name than the one that was looked at, or
    // another text in it since.
    if (meta.dev(), meta.ino()) != seen {
        return Outcome::Other;
    }
    if !job.force {
        match holds(&mut file) {
            Ok(held) if Some(held) == job.known => {}
            Ok(_) => return Outcome::Other,
            Err(error) => return Outcome::Failed(why(&error)),
        }
    }
    // A disk that is full is to show before a byte is written over. That
    // holds where the filesystem can keep room, and no further.
    let length = job.bytes.len() as u64;
    if length > meta.size() {
        // SAFETY: the file is open, and the call takes no memory of ours.
        let kept = unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, length as libc::off_t) };
        if matches!(kept, libc::ENOSPC | libc::EDQUOT | libc::EFBIG) {
            return Outcome::Failed("There is no room for it on the disk.".into());
        }
    }
    let written = (|| {
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&job.bytes)?;
        file.set_len(length)
    })();
    if let Err(error) = written {
        return Outcome::Damaged(why(&error));
    }
    let flushed = file.sync_all().is_ok();
    Outcome::Saved { stamp: confirmed(&mut file, &job.bytes), digest: digest(&job.bytes), flushed }
}

/// Makes a file that is gone anew, and never over something that came
/// into its place meanwhile.
fn anew(job: &Job) -> Outcome {
    anew_with(job, |file| file.write_all(&job.bytes))
}

/// `anew`, with what writes the text into the new file.
fn anew_with(job: &Job, write: impl FnOnce(&mut File) -> std::io::Result<()>) -> Outcome {
    let mut made = OpenOptions::new();
    made.read(true).write(true).create_new(true);
    // With the permissions it had where they are known, and for the user
    // alone until it holds its text: it may be a file of secrets, and
    // what the system gives a new file lets others read it.
    if job.mode.is_some() {
        made.mode(0o600);
    }
    let mut file = match made.open(&job.path) {
        Ok(file) => file,
        Err(error) => return Outcome::Failed(why(&error)),
    };
    // A part of the text under the name of the file is not "nothing was
    // written": the file was made here, and goes again. Where it cannot
    // be taken away, what is left of it is a damaged file.
    if let Err(error) = write(&mut file) {
        return match std::fs::remove_file(&job.path) {
            Ok(()) => Outcome::Failed(why(&error)),
            Err(_) => Outcome::Damaged(why(&error)),
        };
    }
    // Not more than a file is: no bit that makes it run as another. One
    // that cannot be given them stays the user's alone.
    if let Some(mode) = job.mode {
        let _ = file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777));
    }
    let flushed = file.sync_all().is_ok()
        && job.path.parent().is_some_and(|dir| File::open(dir).and_then(|dir| dir.sync_all()).is_ok());
    Outcome::Saved { stamp: confirmed(&mut file, &job.bytes), digest: digest(&job.bytes), flushed }
}

/// Writes a text to its file. For the thread of `Saver`: it waits for the
/// disk.
fn save(job: &Job) -> Outcome {
    // A link is followed to the file: a rename onto the link would put a
    // file in its place.
    let target = match std::fs::canonicalize(&job.path) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if job.force { anew(job) } else { Outcome::Gone };
        }
        Err(error) => return Outcome::Failed(why(&error)),
    };
    // Where the name led when the text was read is where it is written,
    // and nowhere else: a link put under the name since, or one that was
    // turned, leads to a file the user never opened.
    if job.led != 0 && led(&job.path) != job.led {
        return Outcome::Failed("Its name leads to another file now.".into());
    }
    let mut file = match super::open_unblocking(&target) {
        Ok(file) => file,
        Err(error) => return Outcome::Failed(why(&error)),
    };
    let meta = match file.metadata() {
        Ok(meta) if meta.is_file() => meta,
        Ok(meta) if meta.is_dir() => return Outcome::Failed("It is a folder by now.".into()),
        Ok(_) => return Outcome::Failed("It is not a regular file.".into()),
        Err(error) => return Outcome::Failed(why(&error)),
    };
    if !job.force {
        match holds(&mut file) {
            Ok(held) if Some(held) == job.known => {}
            Ok(_) => return Outcome::Other,
            Err(error) => return Outcome::Failed(why(&error)),
        }
    }
    drop(file);
    // A rename asks only the folder, and would replace a file that may
    // not be written to without a word.
    if !may(&target, libc::W_OK) {
        return Outcome::Failed("It may not be written.".into());
    }
    let dir = target.parent().unwrap_or(Path::new("/"));
    if replaceable(&target, dir, &meta) {
        match beside(&target, dir, &meta, &job.bytes) {
            Ok(outcome) => return outcome,
            Err(error) if full(&error) => return Outcome::Failed(why(&error)),
            // Nothing was put in place: the file is written in place
            // instead, which looks at it again first.
            Err(_) => {}
        }
    }
    if !job.in_place {
        return Outcome::InPlace;
    }
    in_place(&target, (meta.dev(), meta.ino()), job)
}

/// Which text an answer is for: the opening of its file, and the mark of
/// the state that was written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Of {
    pub opening: u64,
    pub mark: u64,
}

pub struct Reply {
    pub of: Of,
    pub outcome: Outcome,
}

/// Where texts go to be written, one after another. The thread starts
/// with the first of them, and ends when this is dropped and what it was
/// given is written.
pub struct Saver {
    jobs: Option<mpsc::Sender<(Of, Job)>>,
    reply: Option<Box<dyn Fn(Reply) + Send>>,
}

impl Saver {
    /// `reply` is called on the thread, once for every text that was
    /// handed over. Whether anyone still listens is not asked: the text
    /// is written all the same.
    pub fn new(reply: impl Fn(Reply) + Send + 'static) -> Self {
        Self { jobs: None, reply: Some(Box::new(reply)) }
    }

    /// Hands a text over. Says false if there is no thread to write it:
    /// then no answer comes.
    pub fn send(&mut self, of: Of, job: Job) -> bool {
        if let Some(reply) = self.reply.take() {
            let (jobs, waiting) = mpsc::channel::<(Of, Job)>();
            let work = move || {
                for (of, job) in waiting {
                    // Every text that was handed over is written, also
                    // when nobody listens any more.
                    reply(Reply { of, outcome: save(&job) });
                }
            };
            if std::thread::Builder::new().name("codn save".into()).spawn(work).is_ok() {
                self.jobs = Some(jobs);
            }
        }
        self.jobs.as_ref().is_some_and(|jobs| jobs.send((of, job)).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codn-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn job(path: &Path, was: &str, now: &str) -> Job {
        Job {
            path: path.to_owned(),
            bytes: now.as_bytes().to_vec(),
            known: Some(digest(was.as_bytes())),
            force: false,
            in_place: false,
            led: 0,
            mode: None,
        }
    }

    fn saved(outcome: &Outcome) -> bool {
        matches!(outcome, Outcome::Saved { flushed: true, stamp: Some(_), .. })
    }

    #[test]
    fn a_link_put_under_the_name_since_the_reading_is_not_written_through() {
        let dir = folder("turned");
        let (path, outside) = (dir.join("a.txt"), dir.join("outside.txt"));
        std::fs::write(&path, "one").unwrap();
        let read = led(&path);
        // A program puts a link under the name, to a file of the same
        // bytes: nothing says that the disk holds another text.
        std::fs::write(&outside, "one").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        let turned = Job { led: read, ..job(&path, "one", "mine") };
        assert!(matches!(save(&turned), Outcome::Failed(_)));
        // And to a file of other bytes: the user is told of another
        // text, and his word to write over it is no word for this.
        std::fs::write(&outside, "theirs").unwrap();
        assert!(matches!(save(&Job { force: true, ..turned }), Outcome::Failed(_)));
        assert_eq!(std::fs::read(&outside).unwrap(), b"theirs", "the file the link leads to is as it was");
        // A name that was a link when it was read is written through,
        // as before: the user opened that file by this name.
        let through = Job { led: led(&path), ..job(&path, "theirs", "mine") };
        assert!(saved(&save(&through)));
        assert_eq!(std::fs::read(&outside).unwrap(), b"mine");
        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink(), "the link stays one");
        // Where it is not known where the name led, nothing is refused.
        assert!(saved(&save(&job(&path, "mine", "again"))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_went_and_is_made_anew_has_the_permissions_it_had() {
        let dir = folder("anew-mode");
        let path = dir.join("secret.txt");
        // It went, and the user said to write it all the same.
        let made = |mode| {
            let _ = std::fs::remove_file(&path);
            assert!(saved(&save(&Job { force: true, mode, ..job(&path, "", "all of it") })));
            std::fs::metadata(&path).unwrap().mode() & 0o7777
        };
        assert_eq!(made(Some(0o600)), 0o600, "not what the umask gives, which others may read");
        assert_eq!(made(Some(0o640)), 0o640);
        // Not more than a file is: no bit that makes it run as another.
        assert_eq!(made(Some(0o4755)), 0o755);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_of_ones_own_is_written_beside_itself_and_keeps_what_it_may() {
        let dir = folder("beside");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let before = std::fs::metadata(&path).unwrap();
        let outcome = save(&job(&path, "one", "two\n"));
        let after = std::fs::metadata(&path).unwrap();
        assert!(saved(&outcome), "{outcome:?}");
        assert_eq!(std::fs::read(&path).unwrap(), b"two\n");
        assert_ne!(before.ino(), after.ino(), "a new file under the name");
        assert_eq!(after.mode() & 0o7777, 0o640);
        // The state the answer names is the state the file is in, so that
        // the next look at it does not call the own save a change.
        let Outcome::Saved { stamp: named, digest: held, .. } = outcome else { unreachable!() };
        assert_eq!((named, held), (Some(stamp(&after)), digest(b"two\n")));
        // Nothing is left beside it.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_another_wrote_to_or_took_away_is_left_alone() {
        let dir = folder("other");
        let path = dir.join("a.txt");
        std::fs::write(&path, "theirs").unwrap();
        assert_eq!(save(&job(&path, "one", "mine")), Outcome::Other);
        // The same length, another text: the size does not tell.
        std::fs::write(&path, "eno").unwrap();
        assert_eq!(save(&job(&path, "one", "mine")), Outcome::Other);
        assert_eq!(std::fs::read(&path).unwrap(), b"eno");
        // Said to write all the same, it is written.
        assert!(saved(&save(&Job { force: true, ..job(&path, "one", "mine") })));
        assert_eq!(std::fs::read(&path).unwrap(), b"mine");
        // Gone: nothing is made without a word, and with it the file is
        // made anew, but not over what took its place.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(save(&job(&path, "mine", "again")), Outcome::Gone);
        assert!(!path.exists());
        assert!(saved(&save(&Job { force: true, ..job(&path, "mine", "again") })));
        assert_eq!(std::fs::read(&path).unwrap(), b"again");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let folder = save(&Job { force: true, ..job(&path, "again", "x") });
        assert_eq!(folder, Outcome::Failed("It is a folder by now.".into()));
        // A link to nothing is not a place to make a file at.
        let link = dir.join("link");
        symlink(dir.join("nowhere"), &link).unwrap();
        assert!(matches!(save(&Job { force: true, ..job(&link, "", "x") }), Outcome::Failed(_)));
        assert!(!dir.join("nowhere").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_state_an_answer_names_is_that_of_the_bytes_that_were_written() {
        let dir = folder("confirmed");
        let path = dir.join("a.txt");
        std::fs::write(&path, "mine").unwrap();
        let mut file = File::options().read(true).write(true).open(&path).unwrap();
        let state = stamp(&file.metadata().unwrap());
        assert_eq!(confirmed(&mut file, b"mine"), Some(state));
        // Another wrote into the file right after it was put in place:
        // its state is not kept as that of the own text.
        std::fs::write(&path, "another").unwrap();
        assert_eq!(confirmed(&mut file, b"mine"), None);
        // Also where the size does not tell.
        std::fs::write(&path, "nime").unwrap();
        assert_eq!(confirmed(&mut file, b"mine"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_made_anew_that_could_not_be_written_whole_is_not_left_as_a_part() {
        let dir = folder("anew");
        let path = dir.join("a.txt");
        let job = Job { force: true, ..job(&path, "", "all of the text") };
        let full = |file: &mut File| {
            file.write_all(b"all of")?;
            Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
        };
        assert_eq!(anew_with(&job, full), Outcome::Failed("There is no room for it on the disk.".into()));
        assert!(!path.exists(), "nothing is left under the name");
        assert!(saved(&anew(&job)));
        assert_eq!(std::fs::read(&path).unwrap(), b"all of the text");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_link_stays_a_link_and_a_file_of_two_names_stays_one_file() {
        let dir = folder("links");
        std::fs::create_dir(dir.join("real")).unwrap();
        let (file, link) = (dir.join("real/a.txt"), dir.join("a.txt"));
        std::fs::write(&file, "one").unwrap();
        symlink("real/a.txt", &link).unwrap();
        assert!(saved(&save(&job(&link, "one", "two"))));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "written beside the file, not the link");

        // A second name for the file: a rename would leave it the old text.
        let second = dir.join("second.txt");
        std::fs::hard_link(&file, &second).unwrap();
        let inode = std::fs::metadata(&file).unwrap().ino();
        assert_eq!(save(&job(&file, "two", "a longer text")), Outcome::InPlace);
        assert_eq!(std::fs::read(&file).unwrap(), b"two", "not before the user was told");
        let told = |was: &str, now: &str| Job { in_place: true, ..job(&file, was, now) };
        assert!(saved(&save(&told("two", "a longer text"))));
        assert_eq!(std::fs::read(&second).unwrap(), b"a longer text");
        assert_eq!(std::fs::metadata(&file).unwrap().ino(), inode);
        // A shorter text leaves nothing of the longer one behind it.
        assert!(saved(&save(&told("a longer text", "x"))));
        assert_eq!(std::fs::read(&second).unwrap(), b"x");
        // In place too, a file that holds another text is left alone.
        std::fs::write(&second, "theirs").unwrap();
        assert_eq!(save(&told("x", "mine")), Outcome::Other);
        assert_eq!(std::fs::read(&file).unwrap(), b"theirs");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_may_not_be_written_is_not_replaced_in_silence() {
        let dir = folder("readonly");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        // Who may do everything is not asked: nothing to test then.
        if may(&path, libc::W_OK) {
            return std::fs::remove_dir_all(dir).unwrap();
        }
        assert_eq!(save(&job(&path, "one", "two")), Outcome::Failed("It may not be written.".into()));
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        // In a folder that takes no new file, a file that may be written
        // is written in place.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(save(&job(&path, "one", "two")), Outcome::InPlace);
        assert!(saved(&save(&Job { in_place: true, ..job(&path, "one", "two") })));
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_text_handed_over_is_written_also_when_nobody_listens() {
        let dir = folder("unheard");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one").unwrap();
        let (answers, heard) = mpsc::channel();
        let mut saver = Saver::new(move |reply| drop(answers.send(reply)));
        drop(heard);
        assert!(saver.send(Of { opening: 1, mark: 7 }, job(&path, "one", "two")));
        assert!(saver.send(Of { opening: 1, mark: 8 }, job(&path, "two", "three")));
        drop(saver);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::fs::read(&path).unwrap() != b"three" {
            assert!(std::time::Instant::now() < until, "the second text was not written");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn texts_handed_over_are_written_one_after_another_and_answered() {
        let dir = folder("thread");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one").unwrap();
        let (answers, heard) = mpsc::channel();
        let mut saver = Saver::new(move |reply| drop(answers.send(reply)));
        assert!(saver.send(Of { opening: 1, mark: 7 }, job(&path, "one", "two")));
        assert!(saver.send(Of { opening: 1, mark: 8 }, job(&path, "two", "three")));
        let wait = std::time::Duration::from_secs(10);
        let (first, second) = (heard.recv_timeout(wait).unwrap(), heard.recv_timeout(wait).unwrap());
        assert!(first.of == Of { opening: 1, mark: 7 } && saved(&first.outcome));
        assert!(second.of == Of { opening: 1, mark: 8 } && saved(&second.outcome));
        assert_eq!(std::fs::read(&path).unwrap(), b"three");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
