//! The conversations an agent keeps on disk: finding those of a folder, and
//! asking which one a running agent is in.
//!
//! Nothing in here knows an agent. Where its files lie and which keys in
//! them say what is configuration (`History`), with defaults for the agents
//! codn knows. The files are JSON lines in formats that belong to the
//! agents and may change, so they are searched for keys rather than parsed,
//! and whatever does not fit is left out without complaint.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use super::config::expand;

/// Files looked at per agent, newest first.
const MAX_FILES: usize = 2000;
/// Conversations listed per agent.
const MAX_LISTED: usize = 200;
/// How much of the start of a file is searched for what identifies it.
const HEAD: u64 = 64 * 1024;
/// How much of the end of a file is searched for its title.
const TAIL: u64 = 256 * 1024;
/// How much of the end of an index file is read.
const INDEX: u64 = 8 * 1024 * 1024;
/// Longest title, in characters.
const MAX_TITLE: usize = 120;

/// IDs are data passed to a CLI, never command-line switches. One is a
/// whole argument, or stands in a text that a configuration puts around
/// it: it holds nothing that ends a text or begins something in one.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 1024
        && !id.starts_with('-')
        && !id.chars().any(|c| c.is_control() || c.is_whitespace() || "\"'\\$`".contains(c))
}

/// Filesystems may block even regular-file reads. Keep such workers from
/// accumulating as the user keeps opening lists or typing into sessions.
pub fn spawn(task: impl FnOnce() + Send + 'static) -> bool {
    static WORKERS: AtomicUsize = AtomicUsize::new(0);
    super::spawn_bounded(&WORKERS, "codn history", task)
}

/// Where an agent keeps its conversations and how to read them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct History {
    /// The files, one per conversation. `*` stands for any part of a name,
    /// `{folder}` for the project folder with every character that is not a
    /// letter or digit replaced by `-`.
    pub files: String,
    /// The key whose value is the id the agent resumes a conversation by.
    pub id: String,
    /// The key whose value is the folder a conversation ran in. Without it
    /// every file that `files` matches belongs to the project.
    pub folder: String,
    /// The keys that hold a title, the preferred one first. The last value
    /// in a file counts.
    pub title: Vec<String>,
    /// A file with one line per conversation, naming it by `id`, that is
    /// searched for titles before the conversation itself.
    pub index: String,
    /// Keys and the values one of which a conversation must have.
    pub only: BTreeMap<String, Vec<String>>,
    /// The file a running agent keeps about itself, `{pid}` being its
    /// process id. It holds, under `id`, the conversation the agent is in.
    pub running: String,
}

/// A conversation found on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Conversation {
    pub agent: String,
    pub id: String,
    pub title: String,
    pub changed: SystemTime,
}

impl History {
    /// What is missing for it to be usable, if anything.
    pub(super) fn problem(&self) -> Option<&'static str> {
        if self.files.is_empty() || self.id.is_empty() {
            return Some("a history needs files and id");
        }
        None
    }

    /// The conversations of `folder` that have a title, newest first.
    pub fn list(&self, agent: &str, folder: &Path) -> Vec<Conversation> {
        let index = self.index_titles();
        let mut found = Vec::new();
        for (changed, path) in self.candidates(folder) {
            let Some((id, head)) = self.identify(&path, folder) else { continue };
            let title = index.get(&id).cloned().or_else(|| self.title_in(&path, &head));
            if let Some(title) = title {
                found.push(Conversation { agent: agent.to_owned(), id, title, changed });
                if found.len() == MAX_LISTED {
                    break;
                }
            }
        }
        found
    }

    /// The conversation the agent running as process `pid` is in.
    pub fn current(&self, pid: u32) -> Option<String> {
        if self.running.is_empty() {
            return None;
        }
        let path = expand(&self.running.replace("{pid}", &pid.to_string()));
        let text = read_part(&path, HEAD, false)?;
        values(&text, &self.id).next().filter(|id| valid_id(id))
    }

    /// The files that may hold conversations of `folder`, newest first.
    fn candidates(&self, folder: &Path) -> Vec<(SystemTime, PathBuf)> {
        let encoded: String = folder
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let mut files: Vec<(SystemTime, PathBuf)> = glob(&expand(&self.files.replace("{folder}", &encoded)))
            .into_iter()
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok().filter(|meta| meta.is_file())?;
                Some((meta.modified().ok()?, path))
            })
            .collect();
        files.sort_by(|a, b| b.cmp(a));
        files.truncate(MAX_FILES);
        files
    }

    /// The id of the conversation in a file, if it belongs to `folder` and
    /// passes `only`, and the part of the file that was read for it.
    fn identify(&self, path: &Path, folder: &Path) -> Option<(String, String)> {
        let head = read_part(path, HEAD, false)?;
        let first = |key: &str| values(&head, key).next();
        let id = first(&self.id).filter(|id| valid_id(id))?;
        if !self.folder.is_empty() && !self.ran_in(path, &head, folder) {
            return None;
        }
        // The first value of the key decides, whatever it is: a file that
        // begins as something else is not let in by a later line.
        let allowed = |(key, allowed): (&String, &Vec<String>)| {
            found(&head, key).next().flatten().is_some_and(|v| allowed.contains(&v))
        };
        self.only.iter().all(allowed).then_some((id, head))
    }

    /// Whether the conversation in a file ran in `folder`. As a rule the
    /// key says so near the start. A first message of many kilobytes, a
    /// pasted picture for one, pushes it out of the part that is read:
    /// then the end of the file is asked. A long file that names no folder
    /// in either part belongs where `files` put it, if that is by folder.
    fn ran_in(&self, path: &Path, head: &str, folder: &Path) -> bool {
        let here = |ran: String| Path::new(&ran) == folder;
        if let Some(ran) = values(head, &self.folder).next() {
            return here(ran);
        }
        if (head.len() as u64) < HEAD {
            return false;
        }
        let tail = read_part(path, TAIL, true).unwrap_or_default();
        let mut named = values(&tail, &self.folder).peekable();
        if named.peek().is_none() { self.files.contains("{folder}") } else { named.any(here) }
    }

    /// The title a conversation gave itself, searched for at its end.
    fn title_in(&self, path: &Path, head: &str) -> Option<String> {
        let whole = (head.len() as u64) < HEAD;
        let tail = if whole { None } else { read_part(path, TAIL, true) };
        let text = tail.as_deref().unwrap_or(head);
        self.title.iter().find_map(|key| values(text, key).last().and_then(|title| tidy(&title)))
    }

    /// The titles the index file gives, by conversation id.
    fn index_titles(&self) -> HashMap<String, String> {
        let mut titles = HashMap::new();
        if self.index.is_empty() {
            return titles;
        }
        let Some(text) = read_part(&expand(&self.index), INDEX, true) else { return titles };
        for line in text.lines() {
            let title = self.title.iter().find_map(|key| values(line, key).next().and_then(|t| tidy(&t)));
            if let (Some(id), Some(title)) = (values(line, &self.id).next(), title) {
                titles.insert(id, title);
            }
        }
        titles
    }
}

/// Lists the conversations of a folder for several agents, newest first.
pub fn list(agents: &[(String, History)], folder: &Path) -> Vec<Conversation> {
    let mut all: Vec<Conversation> =
        agents.iter().flat_map(|(agent, history)| history.list(agent, folder)).collect();
    all.sort_by_key(|c| std::cmp::Reverse(c.changed));
    all
}

/// Indices of the conversations that contain every word of `query`.
pub fn matching(found: &[Conversation], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    let fits = |c: &Conversation| {
        let text = format!("{} {}", c.title, c.agent).to_lowercase();
        words.iter().all(|word| text.contains(word))
    };
    found.iter().enumerate().filter(|(_, c)| fits(c)).map(|(i, _)| i).collect()
}

/// How long ago something was, in a few characters.
pub fn age(then: SystemTime, now: SystemTime) -> String {
    let minutes = now.duration_since(then).unwrap_or(Duration::ZERO).as_secs() / 60;
    match minutes {
        0 => "now".into(),
        1..60 => format!("{minutes} min"),
        60..1440 => format!("{} h", minutes / 60),
        1440..20160 => format!("{} d", minutes / 1440),
        20160..86400 => format!("{} wk", minutes / 10080),
        86400..525600 => format!("{} mo", minutes / 43200),
        _ => format!("{} y", minutes / 525600),
    }
}

/// The start or the end of a file, at most `limit` bytes of it.
fn read_part(path: &Path, limit: u64, end: bool) -> Option<String> {
    // Opening a FIFO must not wait for a writer. Only regular files are
    // histories, including when a path changes between listing and opening.
    let mut file = super::open_unblocking(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    if end {
        let size = file.metadata().ok()?.len();
        file.seek(SeekFrom::Start(size.saturating_sub(limit))).ok()?;
    }
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    // The cut may fall into a character. What breaks there is not a key.
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The string values that `key` has anywhere in `text`, in order. JSON
/// inside a string is not searched: its quotes are escaped.
fn values<'a>(text: &'a str, key: &str) -> impl Iterator<Item = String> + 'a {
    found(text, key).flatten()
}

/// What `key` has for a value wherever it stands in `text`, in order: the
/// string, or nothing where the value is of another kind, an object for one.
fn found<'a>(text: &'a str, key: &str) -> impl Iterator<Item = Option<String>> + 'a {
    let needle = format!("\"{key}\"");
    let mut from = 0;
    std::iter::from_fn(move || {
        while let Some(at) = text[from..].find(&needle) {
            let start = from + at;
            from = start + needle.len();
            if text[..start].ends_with('\\') {
                continue;
            }
            // Without a colon behind it, this is a value that reads like the key.
            if let Some(value) = text[from..].trim_start().strip_prefix(':') {
                return Some(value.trim_start().strip_prefix('"').and_then(unescape));
            }
        }
        None
    })
}

/// A JSON string from behind its opening quote to its closing one. Nothing
/// if it does not end within the bounded metadata read.
fn unescape(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars();
    let hex = |chars: &mut std::str::Chars| -> Option<u32> {
        u32::from_str_radix(chars.as_str().get(..4)?, 16).ok().inspect(|_| {
            chars.nth(3);
        })
    };
    for _ in 0..HEAD as usize {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' | 'r' | 't' | 'b' | 'f' => out.push(' '),
                'u' => {
                    let mut code = hex(&mut chars)?;
                    // Two escapes for one character beyond the basic plane.
                    if (0xd800..0xdc00).contains(&code) && chars.as_str().starts_with("\\u") {
                        chars.nth(1);
                        let low = hex(&mut chars)?;
                        if !(0xdc00..=0xdfff).contains(&low) {
                            return None;
                        }
                        code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                    }
                    out.push(char::from_u32(code)?);
                }
                other @ ('"' | '\\' | '/') => out.push(other),
                _ => return None,
            },
            other if other.is_control() => return None,
            other => out.push(other),
        }
    }
    // A bound is not a closing quote: never invent a truncated identity.
    None
}

/// A title on one line and of bounded length, or nothing if it is empty.
fn tidy(title: &str) -> Option<String> {
    // Without what is not seen, as a name that a program reports.
    let seen: String = title.chars().filter(|c| !super::reported::steers(*c)).collect();
    let words: Vec<&str> =
        seen.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let line: String = words.join(" ").chars().take(MAX_TITLE).collect();
    (!line.is_empty()).then_some(line)
}

/// The paths a pattern matches. `*` stands for any part of one name.
fn glob(pattern: &Path) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::new()];
    for part in pattern.iter() {
        let part = part.to_string_lossy();
        if !part.contains('*') {
            paths.iter_mut().for_each(|path| path.push(&*part));
            continue;
        }
        let mut next = Vec::new();
        for dir in &paths {
            let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir.as_path() };
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten().take(50 * MAX_FILES) {
                if entry.file_name().to_str().is_some_and(|name| wild(&part, name)) {
                    next.push(entry.path());
                    if next.len() >= 50 * MAX_FILES {
                        break;
                    }
                }
            }
            // A pattern over a huge tree must not take all memory.
            if next.len() >= 50 * MAX_FILES {
                break;
            }
        }
        paths = next;
    }
    paths
}

/// Whether `name` fits a pattern in which `*` stands for anything.
fn wild(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    let mut parts = parts.peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("codn-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn identities_are_complete_and_not_cli_switches() {
        let root = tree("hostile-ids");
        let path = root.join("current.json");
        let h = History { running: path.display().to_string(), id: "id".into(), ..Default::default() };
        for value in ["".to_owned(), "--unsafe".into(), "a".repeat(1200), "a\nline".into()] {
            // Newline escapes are converted to spaces; literal controls must
            // still never be accepted as an identity.
            write(&path, &format!("{{\"id\":\"{value}\"}}"));
            assert!(h.current(1).is_none(), "{value:?}");
        }
        write(&path, &format!("{{\"id\":\"{}", "x".repeat(1200)));
        assert!(h.current(1).is_none());
        for json in [r#"{"id":"\uD800\u0041"}"#, r#"{"id":"\uD800"}"#, r#"{"id":"\x"}"#] {
            write(&path, json);
            assert!(h.current(1).is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn relative_patterns_keep_their_first_component() {
        let root = tree("relative");
        write(&root.join("one.jsonl"), "{}");
        let relative = root.strip_prefix("/").unwrap().join("*.jsonl");
        // Resolve a relative pattern without mutating the process cwd.
        let absolute = Path::new("/").join(&relative);
        assert_eq!(glob(&absolute), vec![root.join("one.jsonl")]);
        assert!(glob(Path::new("Cargo.toml")).contains(&PathBuf::from("Cargo.toml")));
        assert!(glob(Path::new("Cargo.*")).iter().any(|path| path.ends_with("Cargo.toml")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonregular_metadata_does_not_block() {
        let root = tree("fifo");
        let fifo = root.join("current");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        assert!(read_part(&fifo, HEAD, false).is_none());
        assert!(read_part(&root, HEAD, false).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keys_are_found_at_any_depth_but_not_inside_strings() {
        let line =
            r#"{"a":{"id" : "one"},"text":"{\"id\":\"inner\"}","id":"two","n":{"id":7},"id":"th\"rée\n"}"#;
        assert_eq!(values(line, "id").collect::<Vec<_>>(), ["one", "two", "th\"rée "]);
        assert_eq!(values(line, "text").next().unwrap(), r#"{"id":"inner"}"#);
        assert_eq!(values(r#"{"t":"😀 ok"}"#, "t").next().unwrap(), "😀 ok");
        // A value that is cut off is none.
        assert_eq!(values(r#"{"id":"abc"#, "id").next(), None);
        assert_eq!(tidy("  a\n\tb  "), Some("a b".into()));
        assert_eq!(tidy(" \n"), None);
        // A title is words of a model, and shown as a row: nothing in
        // it that is not seen or turns the writing round, as in a name
        // a program reports.
        assert_eq!(tidy("fix\u{202e}txt.\u{200b}rs \u{3164}"), Some("fixtxt.rs".into()));
        assert_eq!(tidy("\u{3164}\u{2800}"), None);
        // An id is one argument of a program, and may stand in a text a
        // configuration puts around it: nothing that ends such a text.
        for id in ["a b", "-x", "", "a\"b", "a'b", "a\\b", "$(x)", "a`b`"] {
            assert!(!valid_id(id), "{id:?}");
        }
        for id in [
            "0199a2b4-7c1e-7f00-9d3a-5e6f7a8b9c0d",
            "ses_abc.def",
            "/srv/pi/sessions/2026/a.jsonl",
            "a=b:c+d@e",
        ] {
            assert!(valid_id(id), "{id:?}");
        }
    }

    #[test]
    fn patterns_match_parts_of_names() {
        assert!(wild("rollout-*.jsonl", "rollout-1.jsonl"));
        assert!(wild("*", "anything"));
        assert!(wild("a*b*c", "a-b-b-c"));
        assert!(!wild("a*b*c", "a-c"));
        assert!(!wild("rollout-*.jsonl", "rollout-1.json"));
        assert!(!wild("exact", "exactly"));
    }

    #[test]
    fn a_store_with_a_folder_per_project_is_listed() {
        let root = tree("per-folder");
        let history = History {
            files: format!("{}/projects/{{folder}}/*.jsonl", root.display()),
            id: "sessionId".into(),
            title: vec!["customTitle".into(), "aiTitle".into(), "lastPrompt".into()],
            running: format!("{}/sessions/{{pid}}.json", root.display()),
            ..History::default()
        };
        let dir = root.join("projects/-p-my-app");
        write(
            &dir.join("a.jsonl"),
            "{\"type\":\"mode\",\"sessionId\":\"a\"}\n{\"aiTitle\":\"Old\"}\n{\"aiTitle\":\"Fix the build\",\"sessionId\":\"a\"}\n{\"lastPrompt\":\"go\"}\n",
        );
        write(
            &dir.join("b.jsonl"),
            "{\"sessionId\":\"b\",\"aiTitle\":\"Auto\"}\n{\"customTitle\":\"Mine\"}\n{\"aiTitle\":\"Auto 2\"}\n",
        );
        write(&dir.join("empty.jsonl"), "{\"sessionId\":\"never-asked\"}\n");
        write(&dir.join("sub/c.jsonl"), "{\"sessionId\":\"c\",\"aiTitle\":\"A helper\"}\n");
        write(&root.join("projects/-p-other/d.jsonl"), "{\"sessionId\":\"d\",\"aiTitle\":\"Elsewhere\"}\n");
        // A title that lies beyond the start of a long conversation.
        let long =
            format!("{{\"sessionId\":\"long\"}}\n{}\n{{\"aiTitle\":\"At the end\"}}\n", "x".repeat(200_000));
        write(&dir.join("long.jsonl"), &long);
        write(&root.join("sessions/41.json"), "{\"pid\":41,\"sessionId\":\"b\",\"cwd\":\"/p/my.app\"}");

        let mut found = history.list("claude", Path::new("/p/my.app"));
        found.sort_by(|a, b| a.id.cmp(&b.id));
        let titles: Vec<(&str, &str)> = found.iter().map(|c| (c.id.as_str(), c.title.as_str())).collect();
        let (current, none) = (history.current(41), history.current(42));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(titles, [("a", "Fix the build"), ("b", "Mine"), ("long", "At the end")]);
        assert_eq!((current.as_deref(), none), (Some("b"), None));
    }

    #[test]
    fn a_store_that_names_the_folder_inside_is_listed() {
        let root = tree("by-key");
        let history = History {
            files: format!("{}/sessions/*/*/rollout-*.jsonl", root.display()),
            id: "id".into(),
            folder: "cwd".into(),
            title: vec!["thread_name".into()],
            index: format!("{}/index.jsonl", root.display()),
            only: BTreeMap::from([("source".to_owned(), vec!["cli".to_owned(), "vscode".to_owned()])]),
            ..History::default()
        };
        let meta = |id: &str, cwd: &str, source: &str| {
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{id}\",\"id\":\"{id}\",\"cwd\":\"{cwd}\",\"source\":{source}}}}}\n{{\"payload\":{{\"id\":\"rs_1\"}}}}\n"
            )
        };
        let dir = root.join("sessions/2026/10");
        write(&dir.join("rollout-1.jsonl"), &meta("one", "/p/app", "\"cli\""));
        write(&dir.join("rollout-2.jsonl"), &meta("two", "/p/app", "\"exec\""));
        write(&dir.join("rollout-3.jsonl"), &meta("three", "/p/app", "{\"subagent\":{}}"));
        write(&dir.join("rollout-4.jsonl"), &meta("four", "/p/application", "\"cli\""));
        write(&dir.join("rollout-5.jsonl"), &meta("five", "/p/app", "\"vscode\""));
        // A helper that begins with its own line and goes on with the line
        // of the conversation it was forked from.
        let forked = meta("seven", "/p/app", "{\"subagent\":{}}") + &meta("one", "/p/app", "\"cli\"");
        write(&dir.join("rollout-7.jsonl"), &forked);
        write(&dir.join("notes.jsonl"), &meta("six", "/p/app", "\"cli\""));
        write(
            &root.join("index.jsonl"),
            "{\"id\":\"one\",\"thread_name\":\"First\"}\n{\"id\":\"one\",\"thread_name\":\"Renamed\"}\n{\"id\":\"two\",\"thread_name\":\"A run\"}\n",
        );

        let listed = history.list("codex", Path::new("/p/app"));
        let mut all: Vec<_> = history
            .candidates(Path::new("/p/app"))
            .into_iter()
            .filter_map(|(_, path)| history.identify(&path, Path::new("/p/app")).map(|(id, _)| id))
            .collect();
        all.sort();
        std::fs::remove_dir_all(&root).unwrap();
        // "five" has no title and is not offered, but it is a conversation.
        assert_eq!(
            listed.iter().map(|c| (c.id.as_str(), c.title.as_str())).collect::<Vec<_>>(),
            [("one", "Renamed")]
        );
        assert_eq!(all, ["five", "one"]);
    }

    #[test]
    fn a_long_first_message_does_not_hide_where_a_conversation_ran() {
        let root = tree("long-first");
        let history = History {
            files: format!("{}/projects/{{folder}}/*.jsonl", root.display()),
            id: "sessionId".into(),
            folder: "cwd".into(),
            title: vec!["aiTitle".into()],
            ..History::default()
        };
        // As Claude Code writes it: the message comes before the folder.
        let conversation = |id: &str, cwd: &str| {
            format!(
                "{{\"type\":\"mode\",\"sessionId\":\"{id}\"}}\n{{\"type\":\"user\",\"message\":\"{}\",\"cwd\":\"{cwd}\",\"sessionId\":\"{id}\"}}\n{{\"aiTitle\":\"Titled\",\"cwd\":\"{cwd}\"}}\n",
                "x".repeat(70_000)
            )
        };
        let dir = root.join("projects/-p-my-app");
        write(&dir.join("here.jsonl"), &conversation("here", "/p/my.app"));
        // Another folder whose name is written the same way.
        write(&dir.join("other.jsonl"), &conversation("other", "/p/my/app"));
        // A long one that names no folder at all is where the pattern put it.
        let unnamed =
            format!("{{\"sessionId\":\"unnamed\"}}\n{}\n{{\"aiTitle\":\"Titled\"}}\n", "x".repeat(70_000));
        write(&dir.join("unnamed.jsonl"), &unnamed);
        // A short one without the folder is read whole, and says nothing.
        write(&dir.join("short.jsonl"), "{\"sessionId\":\"short\",\"aiTitle\":\"Titled\"}\n");

        let mut found: Vec<String> =
            history.list("claude", Path::new("/p/my.app")).into_iter().map(|c| c.id).collect();
        found.sort();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(found, ["here", "unnamed"]);
    }

    #[test]
    fn conversations_are_filtered_by_words_and_ages_are_short() {
        let at = SystemTime::UNIX_EPOCH;
        let c = |agent: &str, title: &str| Conversation {
            agent: agent.into(),
            id: title.into(),
            title: title.into(),
            changed: at,
        };
        let found = [c("claude", "Fix the build"), c("codex", "Review the build"), c("claude", "Colors")];
        assert_eq!(matching(&found, ""), [0, 1, 2]);
        assert_eq!(matching(&found, "BUILD"), [0, 1]);
        assert_eq!(matching(&found, "build codex"), [1]);
        assert_eq!(matching(&found, "nothing"), Vec::<usize>::new());
        let ago = |secs: u64| age(at, at + Duration::from_secs(secs));
        assert_eq!([ago(20), ago(300), ago(7200), ago(3 * 86400)], ["now", "5 min", "2 h", "3 d"]);
        assert_eq!([ago(21 * 86400), ago(90 * 86400), ago(800 * 86400)], ["3 wk", "3 mo", "2 y"]);
        assert_eq!(age(at + Duration::from_secs(5), at), "now", "a clock that went back");
    }
}
