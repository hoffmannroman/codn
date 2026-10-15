//! Git status and diff, read by running `git` on worker threads.

mod parse;
mod requests;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use super::files::{self, Stamp};
use super::{UNREAPED, wait_until};
pub use parse::inside;
use parse::{parse_diff, parse_marks, parse_status, within};
pub use requests::{Reply, Request, Requests, spawn};

/// Untracked files are shown in full, up to these limits.
const MAX_UNTRACKED: usize = 30;
const MAX_UNTRACKED_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub branch: String,
    /// Changed plus untracked files.
    pub changed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// `text` is the path, `old` and `new` count removed and added lines.
    File,
    /// The same for a file that was deleted: there is nothing to show of it.
    Gone,
    Hunk,
    Add,
    Del,
    Context,
    Note,
}

#[derive(Debug)]
pub struct Line {
    pub kind: Kind,
    /// Line numbers in the old and new file, 0 where there is none.
    pub old: u32,
    pub new: u32,
    pub text: String,
}

/// What changed in one file since the last commit, by the lines of the
/// file as it is, counted from nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    /// Runs of lines that are new or changed, each from its first line
    /// to before its last, in the order of the file.
    pub changed: Vec<(u32, u32)>,
    /// Lines before which lines went, in the order of the file. One
    /// behind the last line stands for the end of the file.
    pub gone: Vec<u32>,
    /// The file is new to git: not tracked, or added since the last
    /// commit, also where that commit has a file of this name that was
    /// taken out of the index since.
    pub new: bool,
}

impl Marks {
    pub fn is_changed(&self, line: usize) -> bool {
        let at = self.changed.partition_point(|(_, end)| *end as usize <= line);
        self.changed.get(at).is_some_and(|(start, _)| *start as usize <= line)
    }

    /// Whether lines went right before a line.
    pub fn is_gone_before(&self, line: usize) -> bool {
        self.gone.binary_search_by(|gone| (*gone as usize).cmp(&line)).is_ok()
    }
}

impl Line {
    /// Whether this is the row that names a file, there or deleted.
    pub fn names_file(&self) -> bool {
        matches!(self.kind, Kind::File | Kind::Gone)
    }

    /// A line that says something about the diff and is not part of it.
    fn note(text: impl Into<String>) -> Self {
        Self { kind: Kind::Note, old: 0, new: 0, text: text.into() }
    }
}

/// Settings that stop a repository from making git run programs. codn
/// calls git on its own, in folders that may hold anything, so nothing a
/// repository's own configuration names may be executed.
const HARDENING: [&str; 12] = [
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    // A diff writes the index anew where a file has another time and the
    // same bytes. codn only asks: nothing in the repository is written,
    // and no hook that follows a change of the index has a reason to run.
    "diff.autoRefreshIndex=false",
    "safe.bareRepository=explicit",
    // Show file names as they are instead of octal escapes.
    "core.quotePath=false",
    // Never recurse into submodule worktrees with their own unsafe drivers.
    "diff.submodule=short",
    // The diff is read by the "a/" and "b/" in front of its file names.
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.srcPrefix=a/",
    "diff.dstPrefix=b/",
    // And by its lines of context, which begin with a space also where
    // they are empty, and by names that are told from the top of the
    // repository.
    "diff.suppressBlankEmpty=false",
    "diff.relative=false",
];
/// How long one git call may take, and how much it may print.
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: u64 = 8 * 1024 * 1024;
/// How long all the calls for one status or one diff may take together. A
/// diff with many new files is more than thirty calls.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// With this many killed calls that have not ended, no further one starts.
const MAX_UNREAPED: usize = 6;

/// git for one folder, with every way to run a program switched off.
struct Git<'a> {
    dir: &'a Path,
    /// Settings that disable the filter drivers and the hooks that the
    /// repository defines: the key as git printed it, and the value it
    /// gets.
    filters: Vec<(Vec<u8>, &'static str)>,
    /// When the request these calls belong to runs out of time.
    deadline: Instant,
    safe: bool,
}

impl<'a> Git<'a> {
    fn new(dir: &'a Path) -> Self {
        let mut git =
            Self { dir, filters: Vec::new(), deadline: Instant::now() + REQUEST_TIMEOUT, safe: true };
        // --local alone excludes config.worktree. The effective list names
        // every scope; disable only repository-owned definitions and includes,
        // retaining trusted helpers from the user's system/global config.
        //
        // The whole list, as bytes. Asking git for the keys that match a
        // pattern leaves out names that are not text in the user's locale,
        // and a driver that is not listed would not be switched off.
        let keys =
            git.run_bytes(&["config", "--includes", "--show-scope", "--null", "--name-only", "--list"]);
        let Some(keys) = keys else {
            // If the complete list cannot be read, running status is unsafe.
            git.safe = false;
            return git;
        };
        let fields: Vec<&[u8]> = keys.split(|byte| *byte == 0).filter(|field| !field.is_empty()).collect();
        git.safe = fields.len().is_multiple_of(2);
        let (mut drivers, mut worktree) = (HashSet::new(), false);
        for pair in fields.as_chunks::<2>().0 {
            let key = pair[1];
            if !matches!(pair[0], b"local" | b"worktree") {
                continue;
            }
            worktree |= key == b"core.worktree";
            // A hook that the configuration names (`hook.<name>.command`,
            // with its event) is not one of the folder of hooks, which
            // `core.hooksPath` empties: each is switched off by its name.
            let hook = key.strip_prefix(b"hook.").and_then(|rest| {
                let end = rest.iter().rposition(|byte| *byte == b'.')?;
                Some(&key[..b"hook.".len() + end])
            });
            if let Some(hook) = hook {
                if drivers.insert(hook) {
                    if drivers.len() > 1024 || Instant::now() >= git.deadline {
                        git.safe = false;
                        break;
                    }
                    git.filters.push(([hook, b".enabled"].concat(), "false"));
                }
                continue;
            }
            let helper = [&b".clean"[..], b".smudge", b".process"].into_iter().find(|end| key.ends_with(end));
            let Some(driver) =
                helper.filter(|_| key.starts_with(b"filter.")).map(|end| &key[..key.len() - end.len()])
            else {
                continue;
            };
            if drivers.insert(driver) {
                // Keep both environment size and setup work bounded. A
                // repository defining this many helpers fails closed.
                if drivers.len() > 1024 || Instant::now() >= git.deadline {
                    git.safe = false;
                    break;
                }
                for (name, value) in [("clean", ""), ("smudge", ""), ("process", ""), ("required", "false")] {
                    git.filters.push(([driver, b".", name.as_bytes()].concat(), value));
                }
            }
        }
        // A repository may name any folder as its work tree, and status and
        // diff would then read that folder instead of the project.
        if git.safe && worktree {
            git.safe = git.owns_work_tree();
        }
        git
    }

    /// Whether the work tree git uses here names this repository as its
    /// own, as a submodule's or a separate git directory's does. A folder
    /// that the repository's configuration merely points at does not.
    fn owns_work_tree(&self) -> bool {
        let Some(out) = self.run_bytes(&["rev-parse", "--show-toplevel", "--absolute-git-dir"]) else {
            return false;
        };
        let mut lines = out.split(|byte| *byte == b'\n').map(|line| Path::new(OsStr::from_bytes(line)));
        let (Some(top), Some(git_dir)) = (lines.next(), lines.next()) else { return false };
        let link = top.join(".git");
        let named = match std::fs::metadata(&link) {
            Ok(meta) if meta.is_dir() => link,
            Ok(meta) if meta.is_file() && meta.len() <= 4096 => {
                let Ok(text) = std::fs::read(&link) else { return false };
                let Some(path) = text.strip_prefix(b"gitdir: ") else { return false };
                top.join(OsStr::from_bytes(path.trim_ascii_end()))
            }
            _ => return false,
        };
        matches!((named.canonicalize(), git_dir.canonicalize()), (Ok(named), Ok(used)) if named == used)
    }

    /// Runs git and returns what it printed. `lenient` also accepts exit
    /// code 1, which `diff --no-index` uses for "the files differ".
    fn run(&self, args: &[&str], lenient: bool) -> Option<String> {
        self.run_raw(args, lenient).map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// The same for answers that are compared byte by byte.
    fn run_bytes(&self, args: &[&str]) -> Option<Vec<u8>> {
        self.run_raw(args, false)
    }

    /// git for this folder with `args`, and with everything set that keeps
    /// the repository from having a say in what runs.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(self.dir);
        for setting in HARDENING {
            command.args(["-c", setting]);
        }
        // Not as `-c name=value`: git cuts that at the first "=", and the
        // name of a driver may contain one. The environment keeps name and
        // value apart, and takes names that are not text.
        if !self.filters.is_empty() {
            // Settings the user's own environment passes this way stay.
            let before =
                std::env::var("GIT_CONFIG_COUNT").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
            command.env("GIT_CONFIG_COUNT", (before + self.filters.len()).to_string());
            for (at, (key, value)) in self.filters.iter().enumerate() {
                command.env(format!("GIT_CONFIG_KEY_{}", before + at), OsStr::from_bytes(key));
                command.env(format!("GIT_CONFIG_VALUE_{}", before + at), value);
            }
        }
        command
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped());
        command
    }

    fn run_raw(&self, args: &[&str], lenient: bool) -> Option<Vec<u8>> {
        let limit = self.deadline.saturating_duration_since(Instant::now()).min(TIMEOUT);
        if !self.safe || limit.is_zero() || UNREAPED.load(Ordering::Relaxed) >= MAX_UNREAPED {
            return None;
        }
        let mut child = self.command(args).spawn().ok()?;
        let deadline = Instant::now() + limit;
        // Read on a thread, so a git that hangs (a dead network mount, a
        // lock) or floods can be stopped.
        let stdout = child.stdout.take()?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.take(MAX_OUTPUT).read_to_end(&mut out);
            let _ = tx.send(out);
        });
        let out = rx.recv_timeout(limit);
        let cut = out.as_ref().is_ok_and(|o| o.len() as u64 >= MAX_OUTPUT);
        if out.is_err() || cut {
            let _ = child.kill();
        }
        // The output can end long before the program does, so the time
        // limit covers its exit too.
        let status = wait_until(child, deadline)?;
        // Never accept a truncated safety configuration list.
        let ok = (status.success() || cut || (lenient && status.code() == Some(1)))
            && !(cut && args.first() == Some(&"config"));
        ok.then(|| out.unwrap_or_default())
    }
}

fn status(dir: &Path) -> Option<Info> {
    Git::new(dir)
        .run(&["status", "--porcelain=v2", "--branch", "--ignore-submodules=dirty"], false)
        .and_then(|out| parse_status(&out))
}

/// The changes of a folder, and where the folder lies in its repository:
/// nothing for its top, else the path from there with a slash behind
/// it. Every file is named from the top of the repository.
fn diff(dir: &Path) -> (String, Vec<Line>) {
    let git = Git::new(dir);
    let flags = ["--no-color", "--no-ext-diff", "--no-textconv", "--ignore-submodules=dirty"];
    let prefix = git.run(&["rev-parse", "--show-prefix"], false).unwrap_or_default();
    let prefix = prefix.trim_end_matches('\n').to_owned();
    // Without a first commit there is no HEAD to compare against.
    let mut lines = Vec::new();
    let mut room = if let Some(out) = git.run(&[&["diff", "HEAD"], &flags[..]].concat(), false) {
        parse_diff(&mut lines, &out)
    } else {
        // An unborn repository has no tree to compare to. Keep staged and
        // unstaged changes explicit, including a file modified after git add.
        let mut room = true;
        for (args, label) in [
            (vec!["diff", "--cached"], "Staged changes (no first commit)"),
            (vec!["diff"], "Unstaged changes"),
        ] {
            let out = git.run(&[&args[..], &flags[..]].concat(), false).unwrap_or_default();
            if room && !out.is_empty() {
                lines.push(Line::note(label));
                room = parse_diff(&mut lines, &out);
            }
        }
        room
    };
    let mut notes = Vec::new();
    match within(&mut lines) {
        0 => {}
        outside => {
            notes.push(format!("{outside} files outside the repository are named by its index: not shown"))
        }
    }
    // As bytes: a name need not be text, and made text with other
    // characters in it, it would be the name of no file.
    let untracked = git.run_bytes(&["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    let untracked: Vec<&[u8]> = untracked.split(|byte| *byte == 0).filter(|f| !f.is_empty()).collect();
    let mut looked_at = 0;
    for name in untracked.iter().take(MAX_UNTRACKED) {
        // Nothing more is read once the view is full.
        if !room {
            break;
        }
        looked_at += 1;
        let Ok(file) = std::str::from_utf8(name) else {
            notes.push(format!("{}: new, its name is no text", String::from_utf8_lossy(name)));
            continue;
        };
        // Only plain files are read. A link is not followed: it may point
        // at a pipe or a device that never ends.
        match std::fs::symlink_metadata(dir.join(file)) {
            Ok(meta) if !meta.is_file() => notes.push(format!("{file}: new, not a regular file")),
            Ok(meta) if meta.len() > MAX_UNTRACKED_BYTES => {
                notes.push(format!("{file}: new file, too large to show"))
            }
            Ok(_) => {
                // Named from this folder with a step on the spot in
                // front: a name that is a dash is what comes in to git,
                // and no file.
                let here = format!("./{file}");
                let shown = git
                    .run(&[&["diff", "--no-index"], &flags[..], &["--", "/dev/null", &here]].concat(), true);
                let from = lines.len();
                room = parse_diff(&mut lines, &shown.unwrap_or_default());
                // git names these from the folder it runs in, and the
                // files it knows from the top of the repository.
                for line in lines[from..].iter_mut().filter(|line| line.names_file()) {
                    let named = line.text.strip_prefix("./").unwrap_or(&line.text);
                    line.text = format!("{prefix}{named}");
                }
            }
            Err(_) => {}
        }
    }
    if untracked.len() > looked_at {
        notes.push(format!("{} more new files not shown", untracked.len() - looked_at));
    }
    lines.extend(notes.into_iter().map(Line::note));
    (prefix, lines)
}

/// What changed in one file of a folder since the last commit, as the
/// view of the changes compares: against `HEAD`. Nothing where git has
/// nothing to say of the file: no repository, no commit yet, a name that
/// is no text, a link, a file git ignores.
fn marks(dir: &Path, file: &Path) -> Option<Marks> {
    let name = file.to_str()?;
    // A link is compared by where it points, which is not what is shown.
    if !std::fs::symlink_metadata(dir.join(file)).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let git = Git::new(dir);
    // A filter or an encoding makes git compare another text than the
    // file holds, with other lines: one of the user's own, which stays
    // switched on, and one of the repository, which is switched off and
    // was at work when the commit was made. Its marks would stand at
    // lines of that other text.
    let attributes = git.run(&["check-attr", "-z", "filter", "working-tree-encoding", "--", name], false)?;
    let mut fields = attributes.split('\0');
    while let (Some(_), Some(_), Some(value)) = (fields.next(), fields.next(), fields.next()) {
        if !matches!(value, "unspecified" | "unset") {
            return None;
        }
    }
    // The name is taken as it is: a `*` in it must not match other files.
    let state = git.run(
        &[
            "--literal-pathspecs",
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
            "--",
            name,
        ],
        false,
    )?;
    if state.lines().any(|line| line.starts_with("? ")) {
        return Some(Marks { new: true, ..Marks::default() });
    }
    // Without context, and without hunks that lie near each other made
    // one: a setting of the user's for that would put lines that did not
    // change between the ones that did.
    let flags = [
        "-U0",
        "--inter-hunk-context=0",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=dirty",
    ];
    let out =
        git.run(&[&["--literal-pathspecs", "diff", "HEAD"], &flags[..], &["--", name]].concat(), false)?;
    // What was cut off says nothing of the rest of the file, and its
    // last line may be half a line.
    if out.len() as u64 >= MAX_OUTPUT {
        return None;
    }
    let mut marks = parse_marks(&out)?;
    // All of a new file is new: its lines say nothing one by one.
    if marks.new {
        marks.changed.clear();
    }
    Some(marks)
}

/// The marks of a file together with the state the file was in while git
/// read it: `None` for the state if it moved meanwhile, and the marks may
/// then be those of neither state.
fn file_marks(dir: &Path, file: &Path) -> (Option<Stamp>, Option<Marks>) {
    let path = dir.join(file);
    let before = files::stamp_of(&path);
    let marks = marks(dir, file);
    (before.filter(|before| files::stamp_of(&path) == Some(*before)), marks)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn what_changed_in_a_file_is_told_by_its_lines() {
        let dir = test_repo("marks");
        std::fs::write(
            dir.join("x.txt"),
            "one
two
three
four
five
",
        )
        .unwrap();
        std::fs::write(
            dir.join("we*ird.txt"),
            "a
",
        )
        .unwrap();
        std::fs::write(
            dir.join("weXird.txt"),
            "a
",
        )
        .unwrap();
        std::fs::write(
            dir.join(".gitignore"),
            "ignored.txt
",
        )
        .unwrap();
        assert!(test_git(&dir, &["add", "."]) && test_git(&dir, &["commit", "-qm", "init"]));
        let of = |name: &str| marks(&dir, Path::new(name));
        assert_eq!(of("x.txt"), Some(Marks::default()), "nothing changed");

        // The first line gone, the third changed, two added at the end.
        std::fs::write(
            dir.join("x.txt"),
            "two
THREE
four
five
six
seven
",
        )
        .unwrap();
        let changed = of("x.txt").unwrap();
        assert_eq!((changed.changed.as_slice(), changed.gone.as_slice()), (&[(1, 2), (4, 6)][..], &[0][..]));
        // Staged or not, it is compared with the last commit.
        assert!(test_git(&dir, &["add", "x.txt"]));
        assert_eq!(of("x.txt").unwrap(), changed);
        // Emptied: the lines went before what is left, which is nothing.
        std::fs::write(dir.join("x.txt"), "").unwrap();
        assert_eq!(of("x.txt").unwrap().gone, [0]);

        // New, whether git was told of it or not, is new as a whole.
        std::fs::write(
            dir.join("new.txt"),
            "a
b
",
        )
        .unwrap();
        let new = Marks { new: true, ..Marks::default() };
        assert_eq!(of("new.txt"), Some(new.clone()));
        assert!(test_git(&dir, &["add", "new.txt"]));
        assert_eq!(of("new.txt"), Some(new));
        // What git ignores has no marks, and neither has a link.
        std::fs::write(
            dir.join("ignored.txt"),
            "a
",
        )
        .unwrap();
        assert_eq!(of("ignored.txt"), Some(Marks::default()));
        std::os::unix::fs::symlink("x.txt", dir.join("link.txt")).unwrap();
        assert_eq!(of("link.txt"), None);
        assert_eq!(of("gone.txt"), None);
        // A star in a name is a star, not every file it could stand for.
        std::fs::write(
            dir.join("weXird.txt"),
            "b
",
        )
        .unwrap();
        assert_eq!(of("we*ird.txt"), Some(Marks::default()));
        assert_eq!(of("weXird.txt").unwrap().changed, [(0, 1)]);

        // The state the file was in comes with the marks.
        let (stamp, marks) = file_marks(&dir, Path::new("weXird.txt"));
        assert!(stamp.is_some() && stamp == files::stamp_of(&dir.join("weXird.txt")) && marks.is_some());
        // Outside a repository there is nothing to say.
        let plain = std::env::temp_dir().join(format!("codn-marks-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(
            plain.join("a.txt"),
            "a
",
        )
        .unwrap();
        // The folder of temporary files may lie in a repository itself.
        if !test_git(&plain, &["rev-parse", "--git-dir"]) {
            assert_eq!(marks_of(&plain, "a.txt"), None);
        }
        std::fs::remove_dir_all(plain).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn marks_of(dir: &Path, name: &str) -> Option<Marks> {
        marks(dir, Path::new(name))
    }

    /// A driver the repository defines under `name`, a change that makes
    /// git read the file, and whether the driver's helper ran after `look`.
    fn helper_ran(label: &str, name: &[u8], look: impl FnOnce(&Path)) -> bool {
        let dir = committed(label);
        let marker = dir.join("MARKER");
        let mut config = std::fs::read(dir.join(".git/config")).unwrap();
        let helper = format!("\"]\n\tclean = touch {}; cat\n", marker.display());
        config.extend_from_slice(&[b"[filter \"", name, helper.as_bytes()].concat());
        std::fs::write(dir.join(".git/config"), config).unwrap();
        std::fs::write(dir.join(".gitattributes"), [b"*.txt filter=", name, b"\n"].concat()).unwrap();
        std::fs::write(dir.join("x.txt"), "two\n").unwrap();
        look(&dir);
        let ran = marker.exists();
        std::fs::remove_dir_all(dir).unwrap();
        ran
    }

    #[test]
    fn drivers_with_unusual_names_cannot_run_helpers() {
        // "=" is where git cuts a `-c name=value`, and the two after it
        // are not text in a UTF-8 locale.
        for (label, name) in
            [("equals", &b"a=b"[..]), ("byte", b"\xff"), ("latin", b"caf\xe9"), ("dots", b"a.b.clean")]
        {
            let ran = helper_ran(label, name, |dir| {
                assert!(status(dir).is_some_and(|info| info.changed > 0), "{label}");
                assert!(diff(dir).1.iter().any(|line| line.text == "two"), "{label}");
            });
            assert!(!ran, "git ran the helper of the driver named {label}");
            // The same repository does run it when git is asked plainly.
            assert!(helper_ran(label, name, |dir| assert!(test_git(dir, &["status"]))), "{label}");
        }
    }

    #[test]
    fn a_work_tree_named_by_the_repository_is_not_read() {
        let dir = committed("redirected");
        assert!(status(&dir).is_some());
        let other = dir.with_extension("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("x.txt"), "theirs\n").unwrap();
        assert!(test_git(&dir, &["config", "core.worktree", other.to_str().unwrap()]));
        let (info, (_, lines)) = (status(&dir), diff(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
        assert_eq!(info, None);
        assert!(lines.is_empty(), "another folder was read: {lines:?}");
    }

    #[test]
    fn prefixes_set_by_the_repository_do_not_change_file_names() {
        let dir = committed("prefixes");
        std::fs::write(dir.join("x.txt"), "two\n").unwrap();
        for setting in
            [["diff.noprefix", "true"], ["diff.mnemonicPrefix", "true"], ["diff.dstPrefix", "new/"]]
        {
            assert!(test_git(&dir, &["config", setting[0], setting[1]]));
            let (_, lines) = diff(&dir);
            assert_eq!(lines.first().map(|line| line.text.as_str()), Some("x.txt"), "{setting:?}");
            assert!(test_git(&dir, &["config", "--unset", setting[0]]));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Builds a repository whose own configuration tries to run programs,
    /// A hook that the configuration of a repository names, as git knows
    /// them since 2.54 (`hook.<name>.event` and `.command`), beside the
    /// files in its folder of hooks. `core.hooksPath` does not reach
    /// these, and the hook after a change of the index runs where a diff
    /// writes the index anew, which it does for a file whose time is
    /// another and whose bytes are not.
    #[test]
    fn a_hook_that_the_configuration_names_does_not_run() {
        let git = |dir: &Path, args: &[&str]| {
            let author = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
            let done = Command::new("git").arg("-C").arg(dir).args(author).args(args).output();
            done.is_ok_and(|o| o.status.success())
        };
        let append = |path: &Path, text: &str| {
            let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
            std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
        };
        for place in ["its configuration", "a file that is included", "the configuration of a work tree"] {
            let dir =
                std::env::temp_dir().join(format!("codn-git-hook-{}-{}", place.len(), std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            if !git(&dir, &["init", "-q", "-b", "main"]) {
                return; // No git on this machine.
            }
            std::fs::write(dir.join("a.txt"), "one\n").unwrap();
            std::fs::write(dir.join("same.txt"), "same\n").unwrap();
            assert!(git(&dir, &["add", "."]) && git(&dir, &["commit", "-q", "-m", "init"]));
            let marker = dir.join("MARKER");
            let hook = format!(
                "[hook \"evil\"]\n\tevent = post-index-change\n\tcommand = touch {}\n",
                marker.display()
            );
            match place {
                "its configuration" => append(&dir.join(".git/config"), &hook),
                "a file that is included" => {
                    append(&dir.join(".git/more"), &hook);
                    append(&dir.join(".git/config"), "[include]\n\tpath = more\n");
                }
                _ => {
                    assert!(git(&dir, &["config", "extensions.worktreeConfig", "true"]));
                    append(&dir.join(".git/config.worktree"), &hook);
                }
            }
            std::fs::write(dir.join("a.txt"), "two\n").unwrap();
            // Another time than the index holds and the same bytes: git
            // has to read the file, and would write the index after.
            let same = std::fs::File::options().write(true).open(dir.join("same.txt")).unwrap();
            same.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600)).unwrap();
            let index = std::fs::read(dir.join(".git/index")).unwrap();
            // Switched off by its name, whatever it waits for, and not
            // only kept from the one occasion a diff gave it.
            let off = Git::new(&dir).filters;
            assert!(
                off.iter().any(|(key, value)| key == b"hook.evil.enabled" && *value == "false"),
                "{place}"
            );
            let _ = status(&dir);
            let (_, lines) = diff(&dir);
            let _ = [marks(&dir, Path::new("a.txt")), marks(&dir, Path::new("same.txt"))];
            let ran = marker.exists();
            let written = std::fs::read(dir.join(".git/index")).unwrap() != index;
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(!ran, "git ran a hook named in {place}");
            assert!(!written, "the index was written, with the hook in {place}");
            assert!(lines.iter().any(|line| line.text == "two"), "the changes are read all the same");
        }
    }

    #[test]
    fn new_files_with_names_git_or_text_stumble_over_are_shown_or_said() {
        let git = |dir: &Path, args: &[&str]| {
            let author = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
            let done = Command::new("git").arg("-C").arg(dir).args(author).args(args).output();
            done.is_ok_and(|o| o.status.success())
        };
        let dir = std::env::temp_dir().join(format!("codn-git-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if !git(&dir, &["init", "-q", "-b", "main"]) {
            return; // No git on this machine.
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        assert!(git(&dir, &["add", "."]) && git(&dir, &["commit", "-q", "-m", "init"]));
        // A dash is what git reads as "what comes in", and a name that
        // is no text was left out without a word.
        for name in [&b"-"[..], b"odd\xff.sh", b"plain.sh"] {
            std::fs::write(dir.join(OsStr::from_bytes(name)), "new\n").unwrap();
        }
        let (_, lines) = diff(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let rows = |name: &str| {
            let at = lines.iter().position(|line| line.kind == Kind::File && line.text == name)?;
            let own = lines[at + 1..].iter().take_while(|line| !line.names_file());
            Some(own.filter(|line| line.kind == Kind::Add).map(|line| line.text.as_str()).collect::<Vec<_>>())
        };
        assert_eq!(rows("-"), Some(vec!["new"]), "with its lines");
        assert_eq!(rows("plain.sh"), Some(vec!["new"]));
        let notes: Vec<&str> =
            lines.iter().filter(|l| l.kind == Kind::Note).map(|l| l.text.as_str()).collect();
        assert!(notes.iter().any(|note| note.contains("odd") && note.contains("no text")), "{notes:?}");
    }

    /// Way by way: what a repository can name for git to run. Each is
    /// one that git runs when it is asked plainly, on a machine whose
    /// git knows it, and none is run by what codn asks.
    #[test]
    fn what_a_repository_names_to_run_is_run_by_git_asked_plainly_and_not_by_codn() {
        let git = |dir: &Path, args: &[&str]| {
            let author = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
            let mut asked = Command::new("git");
            asked.arg("-C").arg(dir).args(author).args(args).stdin(Stdio::null());
            asked.output().is_ok_and(|o| o.status.success())
        };
        let append = |path: &Path, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
            std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
        };
        let hook = |path: &Path, run: &str| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("#!/bin/sh\n{run}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        type Make<'a> = &'a dyn Fn(&Path, &str);
        let ways: [(&str, Make); 7] = [
            ("a hook in its folder of hooks", &|dir, run| {
                hook(&dir.join(".git/hooks/post-index-change"), run)
            }),
            ("a folder of hooks it names", &|dir, run| {
                hook(&dir.join("mine/post-index-change"), run);
                append(
                    &dir.join(".git/config"),
                    &format!("[core]\n\thooksPath = {}\n", dir.join("mine").display()),
                );
            }),
            ("a monitor of the files, in a file it includes", &|dir, run| {
                append(&dir.join(".git/more"), &format!("[core]\n\tfsmonitor = {run}\n"));
                append(&dir.join(".git/config"), "[include]\n\tpath = more\n");
            }),
            ("a filter, named in its own attributes", &|dir, run| {
                append(&dir.join(".git/config"), &format!("[filter \"evil\"]\n\tclean = {run}; cat\n"));
                append(&dir.join(".git/info/attributes"), "*.txt filter=evil\n");
            }),
            ("a filter in capital letters", &|dir, run| {
                append(&dir.join(".git/config"), &format!("[FILTER \"Evil\"]\n\tCLEAN = {run}; cat\n"));
                append(&dir.join(".gitattributes"), "*.txt filter=Evil\n");
            }),
            ("a program for every diff", &|dir, run| {
                append(&dir.join(".git/config"), &format!("[diff]\n\texternal = {run}\n"));
            }),
            ("a program that makes text of a file", &|dir, run| {
                append(&dir.join(".git/config"), &format!("[diff \"evil\"]\n\ttextconv = {run}; cat\n"));
                append(&dir.join(".gitattributes"), "*.txt diff=evil\n");
            }),
        ];
        let mut live = 0;
        for (at, (way, make)) in ways.into_iter().enumerate() {
            let dir = std::env::temp_dir().join(format!("codn-git-way-{at}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            if !git(&dir, &["init", "-q", "-b", "main"]) {
                return; // No git on this machine.
            }
            std::fs::write(dir.join("a.txt"), "one\n").unwrap();
            std::fs::write(dir.join("same.txt"), "same\n").unwrap();
            assert!(git(&dir, &["add", "."]) && git(&dir, &["commit", "-q", "-m", "init"]));
            let marker = dir.join("MARKER");
            make(&dir, &format!("touch {}", marker.display()));
            std::fs::write(dir.join("a.txt"), "two\n").unwrap();
            // Another time and the same bytes: git has to read the file.
            let stale = || {
                let same = std::fs::File::options().write(true).open(dir.join("same.txt")).unwrap();
                same.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600)).unwrap();
            };
            stale();
            let _ = status(&dir);
            let _ = diff(&dir);
            let _ = [marks(&dir, Path::new("a.txt")), marks(&dir, Path::new("same.txt"))];
            let ran = marker.exists();
            // And git asked plainly, to see that the way is one.
            stale();
            git(&dir, &["status"]);
            git(&dir, &["diff", "HEAD"]);
            live += usize::from(marker.exists());
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(!ran, "codn made git run a program by {way}");
        }
        // A git that knows none of these ways shows nothing here.
        assert!(live > 0, "git ran nothing when it was asked plainly");
    }

    #[test]
    fn settings_of_a_repository_do_not_change_how_a_diff_is_read() {
        let git = |dir: &Path, args: &[&str]| {
            let author = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
            let done = Command::new("git").arg("-C").arg(dir).args(author).args(args).output();
            done.is_ok_and(|o| o.status.success())
        };
        let dir = std::env::temp_dir().join(format!("codn-git-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        if !git(&dir, &["init", "-q", "-b", "main"]) {
            return; // No git on this machine.
        }
        std::fs::write(dir.join("a.txt"), "1\n\n3\n4\n5\n").unwrap();
        std::fs::write(dir.join("app/b.txt"), "one\n").unwrap();
        assert!(git(&dir, &["add", "."]) && git(&dir, &["commit", "-q", "-m", "init"]));
        // A line of context that is empty is printed without the space
        // it begins with, and names are told from the folder asked in.
        assert!(git(&dir, &["config", "diff.suppressBlankEmpty", "true"]));
        assert!(git(&dir, &["config", "diff.relative", "true"]));
        std::fs::write(dir.join("a.txt"), "1\n\n3\n4\nfive\n").unwrap();
        std::fs::write(dir.join("app/b.txt"), "two\n").unwrap();
        let (_, lines) = diff(&dir);
        let (_, inner) = diff(&dir.join("app"));
        std::fs::remove_dir_all(&dir).unwrap();
        let numbered =
            lines.iter().find(|line| line.kind == Kind::Add && line.text == "five").map(|line| line.new);
        assert_eq!(numbered, Some(5), "the empty line of context is counted");
        let named = |lines: &[Line]| {
            let files = lines.iter().filter(|line| line.kind == Kind::File);
            files.map(|line| line.text.clone()).collect::<Vec<_>>()
        };
        assert_eq!(named(&lines), ["a.txt", "app/b.txt"]);
        assert_eq!(named(&inner), ["a.txt", "app/b.txt"], "from the top of the repository, asked anywhere");
    }

    /// the way a prompt-injected agent or an unpacked archive could leave it.
    #[test]
    fn a_hostile_repository_cannot_run_anything() {
        let git = |dir: &Path, args: &[&str]| {
            let author = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(author)
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        let dir = std::env::temp_dir().join(format!("codn-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if !git(&dir, &["init", "-q", "-b", "main"]) {
            return; // No git on this machine.
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        assert!(git(&dir, &["add", "a.txt"]) && git(&dir, &["commit", "-q", "-m", "init"]));

        let marker = dir.join("MARKER");
        let run = format!("touch {}", marker.display());
        let config = format!(
            "[core]\n\tfsmonitor = {run}\n[filter \"evil\"]\n\tclean = {run}; cat\n\tprocess = {run}\n\
             [diff \"evil\"]\n\ttextconv = {run}; cat\n\tcommand = {run}\n"
        );
        let mut file = std::fs::OpenOptions::new().append(true).open(dir.join(".git/config")).unwrap();
        std::io::Write::write_all(&mut file, config.as_bytes()).unwrap();
        std::fs::write(dir.join(".gitattributes"), "*.txt filter=evil diff=evil\n").unwrap();
        // Same size as before, so status has to look at the content.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();

        let info = status(&dir);
        let (_, lines) = diff(&dir);
        // What changed in one file is asked for by further calls.
        let marked = [marks(&dir, Path::new("a.txt")), marks(&dir, Path::new("new.txt"))];
        let plain = marks(&dir, Path::new(".gitattributes"));
        let ran = marker.exists();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(!ran, "git ran a program named by the repository");
        // A file with a filter has no marks, one without is new.
        assert_eq!(marked, [None, None]);
        assert_eq!(plain, Some(Marks { new: true, ..Marks::default() }));
        assert_eq!(info.map(|i| i.branch), Some("main".into()));
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert!(text.contains(&"two") && text.contains(&"fresh"), "the diff is still shown: {text:?}");
    }

    fn test_repo(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codn-{label}-{}", crate::core::uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(test_git(&dir, &["init", "-q", "-b", "main"]));
        dir
    }

    /// A repository with one file, committed.
    fn committed(label: &str) -> PathBuf {
        let dir = test_repo(label);
        std::fs::write(dir.join("x.txt"), "one\n").unwrap();
        assert!(test_git(&dir, &["add", "."]) && test_git(&dir, &["commit", "-qm", "init"]));
        dir
    }

    fn test_git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn worktree_filter_configuration_cannot_run_helpers() {
        let dir = committed("worktree-filter");
        assert!(test_git(&dir, &["config", "extensions.worktreeConfig", "true"]));
        let helper = format!("touch {}/MARKER; cat", dir.display());
        assert!(test_git(&dir, &["config", "--worktree", "filter.evil.clean", &helper]));
        std::fs::write(dir.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
        std::fs::write(dir.join("x.txt"), "two\n").unwrap();
        assert!(status(&dir).is_some());
        assert!(diff(&dir).1.iter().any(|line| line.text == "two"));
        assert!(!dir.join("MARKER").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn submodule_filters_cannot_run_but_commit_changes_remain_visible() {
        let dir = test_repo("submodule-filter");
        let source = dir.join("source");
        std::fs::create_dir(&source).unwrap();
        assert!(test_git(&source, &["init", "-q"]));
        std::fs::write(source.join("x.txt"), "one\n").unwrap();
        assert!(test_git(&source, &["add", "."]) && test_git(&source, &["commit", "-qm", "init"]));
        assert!(test_git(
            &dir,
            &["-c", "protocol.file.allow=always", "submodule", "add", source.to_str().unwrap(), "sub"]
        ));
        assert!(test_git(&dir, &["commit", "-qam", "init"]));
        let sub = dir.join("sub");
        // Move the child HEAD before adding the hostile filter.
        std::fs::write(sub.join("x.txt"), "two\n").unwrap();
        assert!(test_git(&sub, &["commit", "-qam", "second"]));
        let helper = format!("touch {}/MARKER; cat", dir.display());
        assert!(test_git(&sub, &["config", "filter.evil.clean", &helper]));
        assert!(test_git(&dir, &["config", "diff.submodule", "diff"]));
        std::fs::write(sub.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
        std::fs::write(sub.join("x.txt"), "bad\n").unwrap();
        assert!(status(&dir).is_some_and(|info| info.changed > 0));
        // A submodule names its work tree in its configuration, and that
        // work tree names it back: as a project of its own it has a state.
        assert!(status(&sub).is_some_and(|info| info.changed > 0));
        let (_, lines) = diff(&dir);
        assert!(lines.iter().any(|line| line.text == "sub"));
        assert!(lines.iter().any(|line| line.text.starts_with("Subproject commit ")));
        assert!(!dir.join("MARKER").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unborn_repositories_show_staged_and_unstaged_changes() {
        let dir = test_repo("unborn");
        std::fs::write(dir.join("x.txt"), "staged\n").unwrap();
        assert!(test_git(&dir, &["add", "x.txt"]));
        assert!(diff(&dir).1.iter().any(|line| line.kind == Kind::Add && line.text == "staged"));
        std::fs::write(dir.join("x.txt"), "working\n").unwrap();
        let (_, lines) = diff(&dir);
        assert!(lines.iter().any(|line| line.kind == Kind::Add && line.text == "staged"));
        assert!(lines.iter().any(|line| line.kind == Kind::Add && line.text == "working"));
        assert!(lines.iter().any(|line| line.text == "Unstaged changes"));
        std::fs::remove_file(dir.join("x.txt")).unwrap();
        assert!(diff(&dir).1.iter().any(|line| line.kind == Kind::Del && line.text == "staged"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn excessive_filter_definitions_fail_closed_without_quadratic_setup() {
        let dir = test_repo("many-filters");
        let mut config = std::fs::read_to_string(dir.join(".git/config")).unwrap();
        for driver in 0..1100 {
            config.push_str(&format!("[filter \"f{driver}\"]\nclean = cat\n"));
        }
        std::fs::write(dir.join(".git/config"), config).unwrap();
        let begun = Instant::now();
        let git = Git::new(&dir);
        assert!(!git.safe && git.run(&["status", "--porcelain=v2"], false).is_none());
        assert!(begun.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_and_filters_do_not_put_marks_at_other_lines() {
        let dir = test_repo("marks-settings");
        let text = |change: &[usize]| {
            let line =
                |n: usize| if change.contains(&n) { format!("LINE {n}\n") } else { format!("line {n}\n") };
            (1..=12).map(line).collect::<String>()
        };
        std::fs::write(dir.join("x.txt"), text(&[])).unwrap();
        std::fs::write(dir.join("f.txt"), text(&[])).unwrap();
        std::fs::write(dir.join(".gitattributes"), "f.txt filter=strip\n").unwrap();
        assert!(test_git(&dir, &["add", "."]) && test_git(&dir, &["commit", "-qm", "init"]));
        std::fs::write(dir.join("x.txt"), text(&[3, 6])).unwrap();
        std::fs::write(dir.join("f.txt"), text(&[3, 6])).unwrap();
        let of = |name: &str| marks(&dir, Path::new(name));
        let two = Some(Marks { changed: vec![(2, 3), (5, 6)], ..Marks::default() });
        assert_eq!(of("x.txt"), two);
        // Hunks that lie near each other stay two, whatever is set: made
        // one, the lines between them would be marked as changed.
        assert!(test_git(&dir, &["config", "diff.interHunkContext", "10"]));
        assert!(test_git(&dir, &["config", "diff.context", "5"]));
        assert_eq!(of("x.txt"), two);
        // A file that goes through a filter is compared as another text
        // than it holds: it has no marks at all.
        assert_eq!(of("f.txt"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_changes_of_a_folder_inside_a_repository_name_files_from_its_top() {
        let dir = test_repo("prefix");
        std::fs::create_dir_all(dir.join("app/src")).unwrap();
        std::fs::write(dir.join("app/src/a.rs"), "one\n").unwrap();
        std::fs::write(dir.join("top.txt"), "one\n").unwrap();
        std::fs::write(dir.join("app/old.txt"), "one\n").unwrap();
        assert!(test_git(&dir, &["add", "."]) && test_git(&dir, &["commit", "-qm", "init"]));
        std::fs::write(dir.join("app/src/a.rs"), "two\n").unwrap();
        std::fs::write(dir.join("app/new.txt"), "new\n").unwrap();
        std::fs::remove_file(dir.join("app/old.txt")).unwrap();
        let names = |lines: &[Line]| -> Vec<(Kind, String)> {
            lines.iter().filter(|l| l.names_file()).map(|l| (l.kind, l.text.clone())).collect()
        };
        // From the top of the repository there is nothing before a name.
        let (prefix, lines) = diff(&dir);
        assert_eq!(prefix, "");
        assert!(names(&lines).contains(&(Kind::Gone, "app/old.txt".into())));
        // From a folder inside it, a file git knows and one it does not
        // are named alike, and the folder says where it lies.
        let (prefix, lines) = diff(&dir.join("app"));
        assert_eq!(prefix, "app/");
        let named = names(&lines);
        assert!(named.contains(&(Kind::File, "app/src/a.rs".into())), "{named:?}");
        assert!(named.contains(&(Kind::File, "app/new.txt".into())), "{named:?}");
        assert!(named.contains(&(Kind::Gone, "app/old.txt".into())), "{named:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
