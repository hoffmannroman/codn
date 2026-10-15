//! Finds git repositories to offer when adding a project.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::SystemTime;

use super::config::{expand, home};
use super::folder_name;

/// How deep below a root repositories are looked for.
const DEPTH: usize = 4;
/// Upper bound on folders visited, so a huge tree cannot stall the scan.
const MAX_FOLDERS: usize = 20_000;
/// Upper bound on the entries of one folder that are looked at for that.
const MAX_ENTRIES: usize = 500_000;

pub struct Repo {
    pub name: String,
    /// The folder it lies in, with the home directory shortened to `~`.
    pub parent: String,
    pub path: PathBuf,
}

/// Counts the scans that were asked for. A scan that is no longer the
/// latest stops: its answer would replace a newer one.
static LATEST: AtomicU64 = AtomicU64::new(0);

/// Scans that have not ended. One that waits for a folder that does not
/// answer cannot give up, and every opening of the list would add another.
static SCANS: AtomicUsize = AtomicUsize::new(0);

/// Scans on a worker thread and hands the result to `done` there. A scan
/// still running from an earlier call gives up. With two that do not end
/// no further one starts, and the list stays as the last scan left it.
pub fn spawn(roots: Vec<PathBuf>, done: impl FnOnce(Vec<Repo>) + Send + 'static) {
    if SCANS.fetch_add(1, Ordering::AcqRel) >= 2 {
        SCANS.fetch_sub(1, Ordering::AcqRel);
        return;
    }
    let mine = LATEST.fetch_add(1, Ordering::Relaxed) + 1;
    let started = std::thread::Builder::new().name("codn scan".into()).spawn(move || {
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                SCANS.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let _done = Done;
        if let Some(repos) = scan(&roots, || LATEST.load(Ordering::Relaxed) != mine) {
            done(repos);
        }
    });
    if started.is_err() {
        SCANS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Repositories below the roots, most recently touched first. Nothing if
/// `overtaken` says that the result is no longer wanted.
fn scan(roots: &[PathBuf], overtaken: impl Fn() -> bool) -> Option<Vec<Repo>> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    // Level by level: if the limit cuts the scan short, it is the deep
    // folders that are missed, not a whole branch.
    let mut queue: VecDeque<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    let mut visited = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        if overtaken() {
            return None;
        }
        visited += 1;
        if visited > MAX_FOLDERS {
            crate::log::log(format_args!("repository scan stopped after {MAX_FOLDERS} folders"));
            break;
        }
        // git touches `.git` on most operations, so its age says how
        // recently someone worked here.
        if let Ok(meta) = std::fs::metadata(dir.join(".git")) {
            found.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), dir.clone()));
            // A repository is a project as a whole, except a root that
            // happens to be one, like a home directory under version control.
            if depth > 0 {
                continue;
            }
        }
        if depth == DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten().take(MAX_ENTRIES) {
            if overtaken() {
                return None;
            }
            let hidden = entry.file_name().as_encoded_bytes().starts_with(b".");
            // One folder with a million entries must not queue a million
            // paths that the limit above then never visits.
            if !hidden && queue.len() < MAX_FOLDERS && entry.file_type().is_ok_and(|t| t.is_dir()) {
                queue.push_back((entry.path(), depth + 1));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.dedup_by(|a, b| a.1 == b.1);
    Some(found.into_iter().map(|(_, path)| repo(path)).collect())
}

/// The folders in the folder last browsed, with the time it last changed.
/// Each letter typed behind a path asks for the same folder again.
static BROWSED: Mutex<Option<(PathBuf, SystemTime, Vec<String>)>> = Mutex::new(None);

/// The names of the folders in `dir`, read once per change of `dir`.
fn subfolders(dir: &Path) -> Vec<String> {
    let changed = std::fs::metadata(dir).and_then(|meta| meta.modified()).ok();
    let browsed = BROWSED.lock().unwrap();
    if let (Some((path, at, names)), Some(changed)) = (&*browsed, changed)
        && path == dir
        && *at == changed
    {
        return names.clone();
    }
    drop(browsed);
    let names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(MAX_ENTRIES)
        // The entry says what it is. Only a link has to be followed.
        .filter(|e| match e.file_type() {
            Ok(kind) if kind.is_symlink() => e.path().is_dir(),
            Ok(kind) => kind.is_dir(),
            Err(_) => false,
        })
        .filter_map(|e| e.file_name().into_string().ok())
        // The limit is on the folders, not on what lies between them: in
        // a folder full of files the folders would otherwise be missing.
        .take(MAX_FOLDERS)
        .collect();
    *BROWSED.lock().unwrap() = changed.map(|changed| (dir.to_owned(), changed, names.clone()));
    names
}

fn repo(path: PathBuf) -> Repo {
    let name = folder_name(&path);
    let parent = path.parent().unwrap_or(Path::new("/"));
    let parent = match parent.strip_prefix(home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => parent.display().to_string(),
    };
    Repo { name, parent, path }
}

/// What choosing a row of the list does.
#[derive(Clone, PartialEq)]
pub enum Pick {
    /// Add this folder as a project.
    Add(PathBuf),
    /// Go into a folder: the field's text becomes this.
    Open(String),
    /// Continue the earlier conversation at this place in the list of them.
    Resume(usize),
    /// Start a session with the agent at this place among those offered.
    Start(usize),
    /// Take the next choice of the option at this place.
    Option(usize),
    /// Leave the options and go back to the list.
    Back,
    /// Show the earlier conversations that the list holds back.
    More,
}

/// One row of the list shown while adding a project.
#[derive(Clone)]
pub struct Row {
    pub pick: Pick,
    pub label: String,
    /// Shown faintly at the right.
    pub detail: String,
}

/// The rows for the text in the field: repositories matching a filter,
/// or the folders at a path being typed.
pub fn rows(repos: &[Repo], text: &str, added: impl Fn(&Path) -> bool) -> Vec<Row> {
    if is_path(text) {
        // Path browsing touches disk and must run on a worker via `browse`.
        return Vec::new();
    }
    filter(repos, text, added)
        .into_iter()
        .map(|i| &repos[i])
        .map(|r| Row { pick: Pick::Add(r.path.clone()), label: r.name.clone(), detail: r.parent.clone() })
        .collect()
}

/// The folder a path points into, as "add it" row, and its subfolders that
/// match the last, unfinished part of the path.
pub fn browse(text: &str) -> Vec<Row> {
    let (typed_dir, prefix) = match text.rsplit_once('/') {
        Some((dir, prefix)) => (dir, prefix),
        None => (text, ""),
    };
    let dir = expand(&format!("{typed_dir}/"));
    let mut rows = Vec::new();
    if prefix.is_empty() && dir.is_dir() {
        let path = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
        let name = path.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned());
        rows.push(Row { pick: Pick::Add(path), label: format!("Add {name}"), detail: "Enter".into() });
    }
    let wanted = prefix.to_lowercase();
    let mut names: Vec<String> = subfolders(&dir)
        .into_iter()
        .filter(|n| n.to_lowercase().starts_with(&wanted) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    rows.extend(names.into_iter().map(|name| Row {
        pick: Pick::Open(format!("{typed_dir}/{name}/")),
        label: format!("{name}/"),
        detail: String::new(),
    }));
    rows
}

/// Whether the text is a path being typed rather than a filter.
pub fn is_path(text: &str) -> bool {
    text.starts_with(['/', '~'])
}

/// Indices of the repositories matching `query`, best first: names that
/// start with it, names that contain it, paths that contain it, then names
/// that contain its letters in order. Within a group the order stays.
fn filter(repos: &[Repo], query: &str, skip: impl Fn(&Path) -> bool) -> Vec<usize> {
    if is_path(query) {
        return Vec::new();
    }
    let query = query.trim().to_lowercase();
    let rank = |repo: &Repo| {
        let name = repo.name.to_lowercase();
        if name.starts_with(&query) {
            Some(0)
        } else if name.contains(&query) {
            Some(1)
        } else if repo.path.to_string_lossy().to_lowercase().contains(&query) {
            Some(2)
        } else {
            let mut letters = name.chars();
            query.chars().all(|q| letters.any(|c| c == q)).then_some(3)
        }
    };
    let mut matches: Vec<(u8, usize)> = repos
        .iter()
        .enumerate()
        .filter(|(_, repo)| !skip(&repo.path))
        .filter_map(|(i, repo)| Some((rank(repo)?, i)))
        .collect();
    matches.sort();
    matches.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_ranks_prefix_before_substring_before_letters() {
        let repos: Vec<Repo> =
            ["/p/nova", "/p/talon-graph", "/p/reef-bal", "/p/talon", "/work/tal/tool", "/p/my-trail-log"]
                .into_iter()
                .map(|p| repo(p.into()))
                .collect();
        let names = |query: &str| -> Vec<&str> {
            filter(&repos, query, |p| p == Path::new("/p/talon"))
                .iter()
                .map(|&i| repos[i].name.as_str())
                .collect()
        };
        // Everything not yet added, in the given (recency) order.
        assert_eq!(names(""), ["nova", "talon-graph", "reef-bal", "tool", "my-trail-log"]);
        assert_eq!(names("tal"), ["talon-graph", "tool", "my-trail-log"]);
        assert_eq!(names("BAL"), ["reef-bal"]);
        assert_eq!(names("~/p"), Vec::<&str>::new());
    }

    #[test]
    fn browsing_lists_the_folder_and_what_matches_below_it() {
        let root = std::env::temp_dir().join(format!("codn-browse-{}", std::process::id()));
        for dir in ["Alpha", "apps/one", "beta", ".cache"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("a-file"), "").unwrap();
        let at = |rest: &str| -> Vec<String> {
            browse(&format!("{}/{rest}", root.display())).into_iter().map(|r| r.label).collect()
        };
        let folder = root.file_name().unwrap().to_string_lossy().into_owned();
        let (all, some, deeper, hidden) = (at(""), at("a"), at("apps/"), at(".c"));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(all, [format!("Add {folder}"), "Alpha/".into(), "apps/".into(), "beta/".into()]);
        assert_eq!(some, ["Alpha/", "apps/"]);
        assert_eq!(deeper, ["Add apps", "one/"]);
        assert_eq!(hidden, [".cache/"]);
    }

    #[test]
    fn scan_finds_repositories_but_not_those_nested_or_hidden() {
        let root = std::env::temp_dir().join(format!("codn-scan-{}", std::process::id()));
        for dir in ["a/.git", "a/inner/.git", "group/b/.git", ".hidden/c/.git", "plain/sub"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let found = scan(std::slice::from_ref(&root), || false).unwrap();
        let mut names: Vec<String> = found.into_iter().map(|r| r.name).collect();
        assert!(scan(std::slice::from_ref(&root), || true).is_none());
        names.sort();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(names, ["a", "b"]);
    }
}
