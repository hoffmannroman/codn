//! What survives a restart: the project list, open sessions, window size.
//! Written by the app only, to `~/.local/state/codn/state.toml`.

use std::collections::HashSet;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use super::config::{Chosen, state_dir};
use crate::log::log;

const LIMIT: u64 = 16 * 1024 * 1024;
/// How many choices of the kind "this agent in this project, with this"
/// are remembered.
pub const MAX_LAST: usize = 300;
/// How many states that could not be read are kept beside the current one.
const KEPT: usize = 10;

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Saved {
    pub width: f32,
    pub height: f32,
    /// UUID of the session that was in front.
    pub active: Option<String>,
    /// Disambiguates agents whose conversation IDs use the same namespace.
    pub active_agent: Option<String>,
    pub projects: Vec<PathBuf>,
    pub sessions: Vec<SavedSession>,
    /// What was last chosen for an agent in a project, the latest last.
    pub last: Vec<Last>,
    /// Whether the column with the files of a project is open.
    pub tree: bool,
    /// Whether the sidebar is folded to a strip. Named for what is not
    /// the usual, so that a state written before this was known reads as
    /// a sidebar that is whole.
    pub folded: bool,
}

/// What a session of an agent was last started with in a project.
#[derive(Clone, Deserialize, Serialize)]
pub struct Last {
    pub project: PathBuf,
    pub agent: String,
    #[serde(default)]
    pub chosen: Chosen,
}

#[derive(Deserialize, Serialize)]
pub struct SavedSession {
    pub project: PathBuf,
    pub agent: String,
    pub id: String,
    pub name: Option<String>,
    pub title: Option<String>,
    /// The id is one the agent has a conversation under.
    #[serde(default)]
    pub known: bool,
    #[serde(default)]
    pub chosen: Chosen,
}

impl Saved {
    /// Reads the state. Only the instance that holds the lock calls this,
    /// so what an earlier one left half written belongs to nobody.
    pub fn load() -> Self {
        let dir = state_dir();
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten().take(4096) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("state.toml.") && name.ends_with(".tmp") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Self::read(&dir.join("state.toml"))
    }

    fn read(path: &Path) -> Self {
        let file = match super::open_unblocking(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            // There is a state, and it cannot be opened: no permission, for
            // one. The next save would replace it like one that is damaged.
            Err(error) => return Self::kept(path, &error.to_string()),
        };
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            return Self::default();
        }
        let mut bytes = Vec::new();
        let parsed = file.take(LIMIT + 1).read_to_end(&mut bytes).map_err(|e| e.to_string()).and_then(|_| {
            if bytes.len() as u64 > LIMIT {
                return Err("state exceeds 16 MiB".to_owned());
            }
            let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
            toml::from_str::<Self>(text).map_err(|e| e.to_string())
        });
        let mut saved = parsed.unwrap_or_else(|e| Self::kept(path, &e));
        saved.prune_last();
        saved
    }

    /// Retains only the latest choice for each current project and agent.
    fn prune_last(&mut self) {
        let mut seen = HashSet::new();
        // A set of the projects: held against the list for each choice,
        // twenty thousand of both took seconds before the window was there.
        let known: HashSet<&PathBuf> = self.projects.iter().collect();
        self.last.reverse();
        self.last.retain(|last| {
            known.contains(&last.project) && seen.insert((last.project.clone(), last.agent.clone()))
        });
        self.last.truncate(MAX_LAST);
        self.last.reverse();
    }

    /// Moves a state that could not be read out of the way of the next
    /// save, and starts empty. An earlier one that was kept stays: it may
    /// be the only copy of the sessions.
    fn kept(path: &Path, e: &str) -> Self {
        let name = |n: usize| match n {
            0 => path.with_extension("toml.bad"),
            n => path.with_extension(format!("toml.bad.{n}")),
        };
        let kept = (0..KEPT).map(name).find(|kept| std::fs::symlink_metadata(kept).is_err());
        let kept = kept.unwrap_or_else(|| name(KEPT - 1));
        match std::fs::rename(path, &kept) {
            Ok(()) => {
                log(format_args!("{} is unreadable and was moved to {}: {e}", path.display(), kept.display()))
            }
            Err(why) => {
                log(format_args!("{} is unreadable: {e}; could not preserve it: {why}", path.display()))
            }
        }
        Self::default()
    }

    fn save(&self) {
        if let Err(e) = self.write_in(&state_dir()) {
            log(format_args!("could not save the state: {e}"));
        }
    }

    fn write_in(&self, dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let text = toml::to_string(self)?;
        if text.len() as u64 > LIMIT {
            return Err("state exceeds 16 MiB".into());
        }
        // The state names projects and conversations, keep it to the user.
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // Where the folder takes no permissions, the state is saved all the
        // same. The file itself is created for the user alone.
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        // Write to a temporary file first so a crash never leaves half a file.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            "state.toml.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        let result = (|| {
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&tmp, dir.join("state.toml"))?;
            std::fs::File::open(dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(tmp);
        }
        result
    }
}

/// Writes the state on a thread of its own. A write waits for the disk, and
/// opening a session must not wait with it.
pub struct Saver {
    states: Arc<(Mutex<Pending>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Pending {
    latest: Option<Saved>,
    finished: bool,
}

impl Saver {
    pub fn start() -> Self {
        Self::with_writer(|saved| saved.save())
    }

    fn with_writer(mut write: impl FnMut(Saved) + Send + 'static) -> Self {
        let states = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker = states.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let (lock, wake) = &*worker;
                let mut pending = lock.lock().unwrap();
                while pending.latest.is_none() && !pending.finished {
                    pending = wake.wait(pending).unwrap();
                }
                let Some(saved) = pending.latest.take() else { break };
                drop(pending);
                write(saved);
            }
        });
        Self { states, thread: Some(thread) }
    }

    pub fn save(&self, saved: Saved) {
        let (lock, wake) = &*self.states;
        let mut pending = lock.lock().unwrap();
        if !pending.finished {
            pending.latest = Some(saved);
            wake.notify_one();
        }
    }

    /// Writes what is still waiting and ends the thread. Called before the
    /// app exits, so the last state is on disk.
    pub fn finish(&mut self) {
        let (lock, wake) = &*self.states;
        lock.lock().unwrap().finished = true;
        wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn state_is_private_and_ignores_legacy_temp_symlinks() {
        let dir = std::env::temp_dir().join(format!("codn-state-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let victim = dir.join("unrelated");
        std::fs::write(&victim, "preserve").unwrap();
        symlink(&victim, dir.join("state.toml.tmp")).unwrap();
        let saved = Saved { width: 800.0, height: 600.0, ..Default::default() };
        saved.write_in(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "preserve");
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(dir.join("state.toml")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(Saved::read(&dir.join("state.toml")).width, 800.0);
        std::fs::write(dir.join("state.toml"), "bad = [").unwrap();
        assert_eq!(Saved::read(&dir.join("state.toml")).width, 0.0);
        assert!(dir.join("state.toml.bad").exists());
        // A second one that cannot be read does not take the first away.
        std::fs::write(dir.join("state.toml"), "worse = [").unwrap();
        assert_eq!(Saved::read(&dir.join("state.toml")).width, 0.0);
        assert_eq!(std::fs::read_to_string(dir.join("state.toml.bad")).unwrap(), "bad = [");
        assert_eq!(std::fs::read_to_string(dir.join("state.toml.bad.1")).unwrap(), "worse = [");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folded_sidebar_is_kept_and_a_state_written_before_it_reads_as_whole() {
        let dir = std::env::temp_dir().join(format!("codn-state-folded-{}", std::process::id()));
        let saved = Saved { folded: true, tree: true, ..Default::default() };
        saved.write_in(&dir).unwrap();
        let back = Saved::read(&dir.join("state.toml"));
        assert!(back.folded && back.tree);
        std::fs::write(dir.join("state.toml"), "width = 900.0\nheight = 600.0\ntree = true\n").unwrap();
        let old = Saved::read(&dir.join("state.toml"));
        assert!(!old.folded && old.tree && old.width == 900.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_preferences_keep_only_recent_current_projects() {
        let dir = std::env::temp_dir().join(format!("codn-state-last-{}", std::process::id()));
        let mut saved = Saved { projects: vec!["/p/a".into()], ..Default::default() };
        for n in 0..350 {
            saved.last.push(Last {
                project: "/p/a".into(),
                agent: n.to_string(),
                chosen: Default::default(),
            });
        }
        saved.last.push(Last { project: "/p/gone".into(), agent: "gone".into(), chosen: Default::default() });
        saved.write_in(&dir).unwrap();
        let back = Saved::read(&dir.join("state.toml"));
        assert_eq!(back.last.len(), 300);
        assert_eq!(back.last[0].agent, "50");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_slow_writer_keeps_only_the_latest_pending_snapshot() {
        let (started, start) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let results = written.clone();
        let mut saver = Saver::with_writer(move |saved| {
            results.lock().unwrap().push(saved.width);
            if saved.width == 1.0 {
                started.send(()).unwrap();
                wait.recv().unwrap();
            }
        });
        saver.save(Saved { width: 1.0, ..Default::default() });
        start.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        for width in 2..=1000 {
            saver.save(Saved { width: width as f32, ..Default::default() });
        }
        release.send(()).unwrap();
        saver.finish();
        assert_eq!(*written.lock().unwrap(), [1.0, 1000.0]);
    }

    #[test]
    fn oversized_state_does_not_replace_the_previous_file() {
        let dir = std::env::temp_dir().join(format!("codn-state-oversized-{}", std::process::id()));
        Saved { width: 800.0, ..Default::default() }.write_in(&dir).unwrap();
        let huge = Saved { active: Some("x".repeat(LIMIT as usize)), ..Default::default() };
        assert!(huge.write_in(&dir).is_err());
        assert_eq!(Saved::read(&dir.join("state.toml")).width, 800.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn whether_the_column_with_the_files_is_open_survives_the_file() {
        let dir = std::env::temp_dir().join(format!("codn-state-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let saved = Saved {
            tree: true,
            projects: vec!["/p/a".into()],
            sessions: vec![SavedSession {
                project: "/p/a".into(),
                agent: "shell".into(),
                id: "one".into(),
                name: None,
                title: None,
                known: false,
                chosen: Default::default(),
            }],
            last: vec![Last { project: "/p/a".into(), agent: "shell".into(), chosen: Default::default() }],
            ..Default::default()
        };
        saved.write_in(&dir).unwrap();
        let back = Saved::read(&dir.join("state.toml"));
        assert!(back.tree && back.sessions.len() == 1 && back.last.len() == 1);
        // A state from before the column has it folded away.
        std::fs::write(dir.join("state.toml"), "width = 800.0\nheight = 600.0\n").unwrap();
        assert!(!Saved::read(&dir.join("state.toml")).tree);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
