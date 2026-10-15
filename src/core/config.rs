//! User configuration, read once at startup from `~/.config/codn/config.toml`.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use super::history::History;
use super::shell;
use crate::log::log;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: Font,
    pub window: Window,
    /// Lines of scrollback kept per session.
    pub scrollback: usize,
    /// Folders searched for git repositories when adding a project.
    pub roots: Vec<String>,
    /// The programs a session can run, in sidebar order.
    #[serde(rename = "agent", deserialize_with = "described")]
    pub agents: Vec<Agent>,
    /// What went wrong reading the file, to show in the window.
    #[serde(skip)]
    pub error: Option<String>,
    /// The search path of the user's shell, if it differs from the one
    /// codn was started with. Sessions get it as their `PATH`.
    #[serde(skip)]
    pub path: Option<String>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Font {
    /// Terminal font family, empty for the system monospace font.
    pub family: String,
    /// Interface font family, empty for the system sans-serif font.
    pub ui_family: String,
    /// Terminal font size in logical pixels.
    pub size: f32,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Window {
    /// Draw minimize, maximize and close. Tiling window managers have no
    /// use for them.
    pub buttons: bool,
}

#[derive(Clone)]
pub struct Agent {
    pub name: String,
    pub cmd: String,
    /// One or two characters shown in front of the agent's sessions.
    pub mark: String,
    /// The name of an icon to show in place of the mark.
    pub icon: Option<String>,
    /// Arguments passed on every start.
    pub args: Vec<String>,
    /// Extra arguments for a new session. `{id}` is the session's UUID.
    pub start: Vec<String>,
    /// Extra arguments to continue a session after a restart of codn.
    pub resume: Vec<String>,
    /// Send Shift+Enter as Alt+Enter, which agent CLIs read as "new line",
    /// to a program that has not switched on the kitty keyboard protocol.
    pub shift_enter: bool,
    /// Where the agent keeps its conversations, if codn is to find them.
    pub history: Option<History>,
    /// What can be set before a session starts, like the model.
    pub options: Vec<Opt>,
    /// Whether the command was found. Missing agents are not offered.
    pub installed: bool,
}

/// An agent as the configuration file says it. One that is named like a
/// built-in agent is that agent, with what the file says in place of what
/// is built in: a file that copied the rest would keep it as it was on
/// the day of the copy, and miss every model built in since. Any other
/// has nothing but what the file says.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Described {
    name: String,
    cmd: Option<String>,
    mark: Option<String>,
    icon: Option<String>,
    args: Option<Vec<String>>,
    start: Option<Vec<String>>,
    resume: Option<Vec<String>>,
    shift_enter: Option<bool>,
    history: Option<History>,
    #[serde(rename = "option")]
    options: Option<Vec<Opt>>,
}

impl From<Described> for Agent {
    fn from(said: Described) -> Self {
        let built =
            built_in(&said.name).unwrap_or_else(|| Agent { icon: None, ..builtin(&said.name, "", "", "") });
        Agent {
            name: said.name,
            // From the home where it says so, as the folders may.
            cmd: said.cmd.map_or(built.cmd, |cmd| match cmd.starts_with("~/") {
                true => expand(&cmd).to_string_lossy().into_owned(),
                false => cmd,
            }),
            mark: said.mark.unwrap_or(built.mark),
            icon: said.icon.or(built.icon),
            args: said.args.unwrap_or(built.args),
            start: said.start.unwrap_or(built.start),
            resume: said.resume.unwrap_or(built.resume),
            shift_enter: said.shift_enter.unwrap_or(built.shift_enter),
            history: said.history.or(built.history),
            options: said.options.unwrap_or(built.options),
            installed: false,
        }
    }
}

fn described<'de, D: Deserializer<'de>>(file: D) -> Result<Vec<Agent>, D::Error> {
    Ok(Vec::<Described>::deserialize(file)?.into_iter().map(Agent::from).collect())
}

/// Something to choose before a session starts. The first choice is what
/// applies when nothing was chosen.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opt {
    pub name: String,
    pub choices: Vec<Choice>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub name: String,
    /// The arguments that make the agent do this. None leaves it to the
    /// agent's own configuration.
    #[serde(default)]
    pub args: Vec<String>,
}

/// What was chosen for a session: the name of an option and of its choice.
pub type Chosen = BTreeMap<String, String>;

impl Default for Font {
    fn default() -> Self {
        Self { family: String::new(), ui_family: String::new(), size: 14.0 }
    }
}

impl Default for Window {
    fn default() -> Self {
        Self { buttons: true }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: Font::default(),
            window: Window::default(),
            scrollback: 10_000,
            roots: vec!["~".into()],
            agents: ["claude", "codex", "shell"].into_iter().filter_map(built_in).collect(),
            error: None,
            path: None,
        }
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

/// An agent that is there without a config file, with nothing but what it
/// is called and shown by.
fn builtin(name: &str, cmd: &str, mark: &str, icon: &str) -> Agent {
    Agent {
        name: name.into(),
        cmd: cmd.into(),
        mark: mark.into(),
        // For Claude Code and Codex the marks of their makers, in place
        // of drawings of codn's choosing.
        icon: Some(icon.into()),
        args: Vec::new(),
        start: Vec::new(),
        resume: Vec::new(),
        shift_enter: false,
        history: None,
        options: Vec::new(),
        installed: false,
    }
}

/// An option whose choices are the values of one argument, after
/// "default", which passes nothing and leaves it to the agent.
fn option(name: &str, before: &[&str], values: &[&str]) -> Opt {
    Opt {
        name: name.into(),
        choices: std::iter::once(Choice { name: "default".into(), args: Vec::new() })
            .chain(values.iter().map(|value| Choice {
                name: value.to_string(),
                args: before.iter().map(|arg| arg.replace("{}", value)).collect(),
            }))
            .collect(),
    }
}

// The models and efforts are those the two programs offered in October
// 2026: Claude Code 2.1 by its names for models, Codex 0.160 by its list of
// models. A new one is a line in the config file until it is a default
// here. Not every model of Codex takes every effort; which does, Codex says
// when it starts.
//
// The histories are what is known of the two agents' own files. They may
// change them; a `[agent.history]` in the config file then says it anew.

fn claude() -> Agent {
    Agent {
        start: strings(&["--session-id", "{id}"]),
        resume: strings(&["--resume", "{id}"]),
        shift_enter: true,
        history: Some(History {
            files: "~/.claude/projects/{folder}/*.jsonl".into(),
            id: "sessionId".into(),
            folder: "cwd".into(),
            title: strings(&["customTitle", "aiTitle", "lastPrompt"]),
            running: "~/.claude/sessions/{pid}.json".into(),
            ..History::default()
        }),
        options: vec![
            option(
                "Model",
                &["--model", "{}"],
                &["fable", "opus", "opus[1m]", "sonnet", "haiku", "opusplan"],
            ),
            option("Effort", &["--effort", "{}"], &["low", "medium", "high", "xhigh", "max"]),
            option("Permissions", &["--permission-mode", "{}"], &["manual", "acceptEdits", "auto", "plan"]),
        ],
        ..builtin("claude", "claude", "✳", "agent")
    }
}

fn codex() -> Agent {
    Agent {
        resume: strings(&["resume", "{id}"]),
        shift_enter: true,
        history: Some(History {
            files: "~/.codex/sessions/*/*/*/rollout-*.jsonl".into(),
            id: "id".into(),
            folder: "cwd".into(),
            title: strings(&["thread_name"]),
            index: "~/.codex/session_index.jsonl".into(),
            only: [("source".to_owned(), strings(&["cli", "vscode"]))].into(),
            ..History::default()
        }),
        options: vec![
            option(
                "Model",
                &["-m", "{}"],
                &[
                    "gpt-6.1-sol",
                    "gpt-6-astra",
                    "gpt-6-sol",
                    "gpt-6-luna",
                    "gpt-5.6-sol",
                    "gpt-5.6-terra",
                    "gpt-5.6-luna",
                    "gpt-5.5",
                ],
            ),
            option(
                "Effort",
                &["-c", "model_reasoning_effort={}"],
                &["low", "medium", "high", "xhigh", "max", "ultra"],
            ),
            option("Sandbox", &["--sandbox", "{}"], &["read-only", "workspace-write"]),
        ],
        ..builtin("codex", "codex", ">_", "agent")
    }
}

fn shell() -> Agent {
    builtin("shell", &super::shell::name().unwrap_or_else(|| "/bin/sh".into()), "$", "terminal")
}

/// The agent that is there without a config file under this name.
fn built_in(name: &str) -> Option<Agent> {
    match name {
        "claude" => Some(claude()),
        "codex" => Some(codex()),
        "shell" => Some(shell()),
        _ => None,
    }
}

/// How long a mistake of the configuration is said at the most. It is
/// wrapped anew for every frame, and what is said of a value of the
/// wrong kind holds that value: half a megabyte of it once.
const MAX_MISTAKE: usize = 300;

/// A mistake of the configuration as the window says it: one short line.
fn mistake(said: &str) -> String {
    let mut line: String = "config.toml: ".into();
    let plain = said.chars().map(|c| if c.is_control() { ' ' } else { c });
    line.extend(plain.take(MAX_MISTAKE));
    if said.chars().nth(MAX_MISTAKE).is_some() {
        line.push('\u{2026}');
    }
    line
}

impl Config {
    pub fn load() -> Self {
        let path = config_dir().join("config.toml");
        let mut config = match Self::read(&path) {
            Ok(config) => config,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log(format_args!("{}: {e}", path.display()));
                // The defaults run, and the window says why.
                Self { error: Some(mistake(&e.to_string())), ..Self::default() }
            }
        };
        config.find_agents();
        config
    }

    fn read(path: &Path) -> std::io::Result<Self> {
        let file = super::open_unblocking(path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("configuration is not a regular file"));
        }
        const LIMIT: u64 = 1024 * 1024;
        let mut text = String::new();
        file.take(LIMIT + 1).read_to_string(&mut text)?;
        if text.len() as u64 > LIMIT {
            return Err(std::io::Error::other("configuration exceeds 1 MiB"));
        }
        Self::parse(&text).map_err(std::io::Error::other)
    }

    fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| e.message().to_owned())?;
        if !config.font.size.is_finite() || !(8.0..=40.0).contains(&config.font.size) {
            return Err("font.size must be between 8 and 40".into());
        }
        if config.scrollback > 1_000_000 {
            return Err("scrollback must not exceed 1000000 lines".into());
        }
        let mut names = HashSet::new();
        for agent in &config.agents {
            if agent.name.is_empty() || agent.cmd.is_empty() {
                return Err("an agent needs a name and a cmd".into());
            }
            if !names.insert(&agent.name) {
                return Err(format!("two agents are named \"{}\"", agent.name));
            }
            if let Some(problem) = agent.history.as_ref().and_then(History::problem) {
                return Err(format!("agent \"{}\": {problem}", agent.name));
            }
            if let Some(problem) = options_problem(&agent.options) {
                return Err(format!("agent \"{}\": {problem}", agent.name));
            }
        }
        Ok(config)
    }

    /// Marks the agents whose command exists.
    fn find_agents(&mut self) {
        let path = shell::search_path(self.path.as_deref());
        for agent in &mut self.agents {
            agent.installed = shell::which(&agent.cmd, &path);
        }
    }

    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.name == name)
    }
}

impl Agent {
    /// A running process must name its own identity. A file appearing in
    /// the same folder could belong to another process.
    pub fn follows(&self) -> bool {
        self.history.as_ref().is_some_and(|history| !history.running.is_empty())
    }
    /// The full argument list for a fresh or a resumed session. `known`
    /// says that `uuid` is an id the agent has a conversation under,
    /// `chosen` what was set for the session, `status` the file this start
    /// may report to, empty if it has none.
    pub fn arguments(
        &self,
        uuid: &str,
        status: &str,
        resume: bool,
        known: bool,
        chosen: &Chosen,
    ) -> Vec<String> {
        // Resuming by an id the agent never heard of would end at once, or
        // in the wrong conversation.
        let can = !self.resume.is_empty() && (known || self.takes_id() || !names_id(&self.resume));
        let extra = if resume && can { &self.resume } else { &self.start };
        // What was chosen comes before a subcommand like `resume`.
        let chosen = self.options.iter().flat_map(|option| &option.choices[option.chosen(chosen)].args);
        let all = self.args.iter().chain(chosen).chain(extra);
        // What is put in is not read again: an id may hold the name of the
        // file, and a path that of the id.
        let filled = |a: &String| {
            a.split("{status}").map(|part| part.replace("{id}", uuid)).collect::<Vec<_>>().join(status)
        };
        all.map(filled).collect()
    }

    /// Whether a new session is started under an id that codn chooses.
    pub fn takes_id(&self) -> bool {
        names_id(&self.start)
    }

    /// The agent's conversations on disk, if one of them can be resumed.
    pub fn resumable(&self) -> Option<&History> {
        self.history.as_ref().filter(|_| names_id(&self.resume))
    }
}

impl Opt {
    /// The place of what was chosen among the choices. The first one if
    /// nothing was, or something that is no longer offered.
    pub fn chosen(&self, chosen: &Chosen) -> usize {
        let name = chosen.get(&self.name);
        name.and_then(|name| self.choices.iter().position(|choice| choice.name == *name)).unwrap_or(0)
    }
}

/// What was chosen, in a few words: the choices that are not the first of
/// their option.
pub fn summary(options: &[Opt], chosen: &Chosen) -> String {
    let set = options.iter().map(|option| (option, option.chosen(chosen))).filter(|(_, at)| *at > 0);
    set.map(|(option, at)| option.choices[at].name.as_str()).collect::<Vec<_>>().join(" · ")
}

/// What is wrong with the options of an agent, if anything.
fn options_problem(options: &[Opt]) -> Option<&'static str> {
    if options.iter().any(|option| option.name.is_empty() || option.choices.is_empty()) {
        return Some("an option needs a name and choices");
    }
    let mut names = HashSet::new();
    for option in options {
        let mut choices = HashSet::new();
        if !names.insert(&option.name)
            || option.choices.iter().any(|choice| choice.name.is_empty() || !choices.insert(&choice.name))
        {
            return Some("option and choice names must be nonempty and unique");
        }
    }
    None
}

fn names_id(args: &[String]) -> bool {
    args.iter().any(|arg| arg.contains("{id}"))
}

/// Replaces a leading `~` with the home directory.
pub fn expand(text: &str) -> PathBuf {
    match text.strip_prefix('~') {
        Some(rest) => home().join(rest.trim_start_matches('/')),
        None => PathBuf::from(text),
    }
}

pub(super) fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| "/".into(), PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map_or_else(|| home().join(fallback), PathBuf::from)
        .join("codn")
}

fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn the_command_of_an_agent_may_begin_at_the_home_folder() {
        // As the folders of projects and of conversations may: a command
        // is no line of a shell, and nothing else makes a home of `~`.
        let text = "[[agent]]\nname = \"mine\"\ncmd = \"~/bin/agent\"\n[[agent]]\nname = \"odd\"\ncmd = \"~agent\"\n";
        let config = Config::parse(text).unwrap();
        assert_eq!(Path::new(&config.agents[0].cmd), home().join("bin/agent"));
        assert_eq!(config.agents[1].cmd, "~agent", "a name, not a path from the home");
    }

    #[test]
    fn a_mistake_is_said_in_one_short_line_whatever_value_it_was_made_with() {
        // The library names the value that is of the wrong kind, all of it.
        let long = format!("scrollback = \"{}\"\n", "x".repeat(500_000));
        let said = mistake(&Config::parse(&long).err().unwrap());
        assert!(
            said.starts_with("config.toml: ") && said.chars().count() <= MAX_MISTAKE + 14,
            "{}",
            said.len()
        );
        assert!(said.ends_with('\u{2026}'));
        assert_eq!(mistake("two\nlines\tof it"), "config.toml: two lines of it");
        assert_eq!(
            mistake("font.size must be between 8 and 40"),
            "config.toml: font.size must be between 8 and 40"
        );
    }

    #[test]
    fn a_config_file_overrides_what_it_names() {
        let text = "scrollback = 500\n[font]\nsize = 16\n[[agent]]\nname = \"a\"\ncmd = \"/bin/sh\"\nargs = [\"-l\"]\nstart = [\"--new\", \"{id}\"]\n";
        let config = Config::parse(text).unwrap();
        assert_eq!((config.scrollback, config.font.size, config.agents.len()), (500, 16.0, 1));
        assert!(config.window.buttons, "what is not named keeps its default");
        let agent = &config.agents[0];
        let none = Chosen::new();
        assert_eq!(agent.arguments("u1", "", false, false, &none), ["-l", "--new", "u1"]);
        // An agent that cannot resume starts the way it always does.
        assert_eq!(agent.arguments("u1", "", true, false, &none), ["-l", "--new", "u1"]);
        assert!(agent.resumable().is_none());
    }

    #[test]
    fn a_conversation_is_resumed_only_by_an_id_the_agent_knows() {
        let text = "[[agent]]\nname = \"a\"\ncmd = \"a\"\nresume = [\"resume\", \"{id}\"]\n[agent.history]\nfiles = \"~/a/*.jsonl\"\nid = \"id\"\n[[agent]]\nname = \"b\"\ncmd = \"b\"\nresume = [\"--last\"]\n";
        let config = Config::parse(text).unwrap();
        let (a, b) = (&config.agents[0], &config.agents[1]);
        // The id is codn's own until the agent's conversation was found.
        let none = Chosen::new();
        assert_eq!(a.arguments("u1", "", true, false, &none), Vec::<String>::new());
        assert_eq!(a.arguments("u1", "", true, true, &none), ["resume", "u1"]);
        assert!(a.resumable().is_some());
        // Resuming without an id needs none.
        assert_eq!(b.arguments("u1", "", true, false, &none), ["--last"]);
        let defaults = Config::default();
        let claude = defaults.agent("claude").unwrap();
        assert!(claude.follows());
        assert!(!defaults.agent("codex").unwrap().follows());
        assert_eq!(claude.arguments("u1", "", true, false, &none), ["--resume", "u1"], "codn chose the id");
        assert!(claude.resumable().is_some() && defaults.agent("shell").unwrap().resumable().is_none());
        assert!(
            Config::parse("[[agent]]\nname = \"a\"\ncmd = \"a\"\n[agent.history]\nid = \"id\"\n").is_err()
        );
    }

    #[test]
    fn what_was_chosen_becomes_arguments_before_the_subcommand() {
        let text = "[[agent]]\nname = \"a\"\ncmd = \"a\"\nresume = [\"resume\", \"{id}\"]\n[[agent.option]]\nname = \"Model\"\nchoices = [{ name = \"default\" }, { name = \"big\", args = [\"-m\", \"big\"] }]\n[[agent.option]]\nname = \"Effort\"\nchoices = [{ name = \"default\" }, { name = \"high\", args = [\"-c\", \"effort=high\"] }]\n";
        let config = Config::parse(text).unwrap();
        let agent = &config.agents[0];
        let chosen = |pairs: &[(&str, &str)]| -> Chosen {
            pairs.iter().map(|(option, choice)| (option.to_string(), choice.to_string())).collect()
        };
        let both = chosen(&[("Model", "big"), ("Effort", "high")]);
        assert_eq!(
            agent.arguments("u", "", true, true, &both),
            ["-m", "big", "-c", "effort=high", "resume", "u"]
        );
        assert_eq!(summary(&agent.options, &both), "big · high");
        // What is no longer offered counts as not chosen.
        let stale = chosen(&[("Model", "gone"), ("Speed", "fast")]);
        assert_eq!(agent.arguments("u", "", false, false, &stale), Vec::<String>::new());
        assert_eq!(summary(&agent.options, &stale), "");
        // The defaults pass nothing until something is chosen.
        let defaults = Config::default();
        let codex = defaults.agent("codex").unwrap();
        assert_eq!(codex.arguments("u", "", false, false, &Chosen::new()), Vec::<String>::new());
        let high = chosen(&[("Effort", "high")]);
        assert_eq!(codex.arguments("u", "", false, false, &high), ["-c", "model_reasoning_effort=high"]);
        assert!(
            Config::parse(
                "[[agent]]\nname = \"a\"\ncmd = \"a\"\n[[agent.option]]\nname = \"x\"\nchoices = []\n"
            )
            .is_err()
        );
    }

    #[test]
    fn the_file_to_report_to_can_be_an_argument() {
        let text = "[[agent]]\nname = \"a\"\ncmd = \"a\"\nargs = [\"-c\", 'notify=[\"report\", \"{status}\"]']\nstart = [\"--new\", \"{id}\"]\nresume = [\"resume\", \"{id}\"]\n";
        let config = Config::parse(text).unwrap();
        let agent = &config.agents[0];
        let none = Chosen::new();
        assert_eq!(
            agent.arguments("u1", "/state/5-1.toml", false, false, &none),
            ["-c", "notify=[\"report\", \"/state/5-1.toml\"]", "--new", "u1"]
        );
        // After a restart too, and a start without a file names none.
        assert_eq!(
            agent.arguments("u1", "", true, true, &none),
            ["-c", "notify=[\"report\", \"\"]", "resume", "u1"]
        );
        // What is put in is put in as it is, also where a path looks like
        // the name of the id, or an id like that of the file.
        assert_eq!(
            agent.arguments("u1", "/{id}/1.toml", false, false, &none)[1],
            "notify=[\"report\", \"/{id}/1.toml\"]"
        );
        assert_eq!(agent.arguments("{status}", "/s", true, true, &none)[2..], ["resume", "{status}"]);
    }

    #[test]
    fn an_agent_named_like_a_built_in_one_is_that_one_but_for_what_it_says() {
        let text = "[[agent]]\nname = \"shell\"\n\n[[agent]]\nname = \"codex\"\nargs = [\"-c\", \"x={status}\"]\n\
                    [[agent.option]]\nname = \"Model\"\nchoices = [{ name = \"default\" }]\n\n\
                    [[agent]]\nname = \"claude\"\ncmd = \"/opt/claude\"\nmark = \"c\"\nicon = \"star\"\nstart = []\n\
                    resume = [\"-r\"]\nshift_enter = false\n[agent.history]\nfiles = \"~/c/*.jsonl\"\nid = \"id\"\n";
        let config = Config::parse(text).unwrap();
        let built = Config::default();
        // The file says which agents there are, and in which order.
        assert_eq!(
            config.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["shell", "codex", "claude"]
        );
        let same = |a: &Agent, b: &Agent| {
            let pairs = |agent: &Agent| -> Vec<(String, String, Vec<String>)> {
                let all = agent.options.iter().flat_map(|o| o.choices.iter().map(move |c| (o, c)));
                all.map(|(o, c)| (o.name.clone(), c.name.clone(), c.args.clone())).collect()
            };
            (&a.cmd, &a.mark, &a.icon, &a.args, &a.start, &a.resume)
                == (&b.cmd, &b.mark, &b.icon, &b.args, &b.start, &b.resume)
                && (a.shift_enter, &a.history) == (b.shift_enter, &b.history)
                && pairs(a) == pairs(b)
        };
        // A name alone is the agent as it is built in.
        let (shell, codex, claude) = (&config.agents[0], &config.agents[1], &config.agents[2]);
        assert!(same(shell, built.agent("shell").unwrap()));
        // What is said takes the place of what is built in, as a whole: the
        // options are those of the file. What is not said stays.
        assert_eq!(codex.arguments("u", "/s", true, true, &Chosen::new()), ["-c", "x=/s", "resume", "u"]);
        assert_eq!(codex.options.len(), 1);
        assert!(same(
            &Agent {
                args: Vec::new(),
                options: built.agent("codex").unwrap().options.clone(),
                ..codex.clone()
            },
            built.agent("codex").unwrap()
        ));
        // Every key can be said.
        assert_eq!(
            (claude.cmd.as_str(), claude.mark.as_str(), claude.icon.as_deref()),
            ("/opt/claude", "c", Some("star"))
        );
        assert_eq!(
            (&claude.start, &claude.resume, claude.shift_enter),
            (&Vec::new(), &strings(&["-r"]), false)
        );
        assert_eq!(claude.history.as_ref().unwrap().files, "~/c/*.jsonl");
        assert!(claude.history.as_ref().unwrap().running.is_empty(), "a history is said as a whole");
        assert_eq!(claude.options.len(), 3);
        // Any other agent has nothing but what the file says, and needs a command.
        let other = &Config::parse("[[agent]]\nname = \"codex2\"\ncmd = \"codex\"\n").unwrap().agents[0];
        assert!(
            other.options.is_empty()
                && other.history.is_none()
                && other.icon.is_none()
                && other.mark.is_empty()
        );
        assert!(Config::parse("[[agent]]\nname = \"codex2\"\n").err().unwrap().contains("cmd"));
        assert!(
            Config::parse("[[agent]]\nname = \"codex\"\nmodel = \"x\"\n").err().unwrap().contains("model")
        );
    }

    #[test]
    fn mistakes_are_named() {
        assert!(Config::parse("scrolback = 1").err().unwrap().contains("scrolback"));
        let twice = "[[agent]]\nname = \"a\"\ncmd = \"x\"\n[[agent]]\nname = \"a\"\ncmd = \"y\"\n";
        assert!(Config::parse(twice).err().unwrap().contains("two agents"));
        assert!(Config::parse("[[agent]]\nname = \"a\"\ncmd = \"\"\n").is_err());
    }

    #[test]
    fn sizes_and_ambiguous_options_are_rejected() {
        for size in ["nan", "inf", "0", "1000"] {
            assert!(Config::parse(&format!("[font]\nsize = {size}\n")).is_err());
        }
        assert!(Config::parse("scrollback = 1000001").is_err());
        let agent = "[[agent]]\nname = \"a\"\ncmd = \"sh\"\n";
        let option = "[[agent.option]]\nname = \"Model\"\nchoices = [{name = \"same\"}, {name = \"same\"}]\n";
        assert!(Config::parse(&format!("{agent}{option}")).is_err());
        let option = "[[agent.option]]\nname = \"Model\"\nchoices = [{name = \"default\"}]\n";
        assert!(Config::parse(&format!("{agent}{option}{option}")).is_err());
    }

    #[test]
    fn configuration_reads_are_bounded_and_skip_nonregular_files() {
        let root = std::env::temp_dir().join(format!("codn-config-read-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("config.toml");
        std::fs::write(&file, " ".repeat(1024 * 1024 + 1)).unwrap();
        assert!(Config::read(&file).is_err());
        assert!(Config::read(&root).is_err());
        let fifo = root.join("fifo");
        assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        assert!(Config::read(&fifo).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
