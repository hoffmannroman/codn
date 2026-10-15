# The codn manual

How to use codn, what it reads and runs, and how to configure it. What
codn is, and how to build it, is in the [README](README.md).

- [Use](#use)
- [The bar along the bottom](#the-bar-along-the-bottom)
- [Configure](#configure)
- [What codn runs on its own](#what-codn-runs-on-its-own)
- [The code](#the-code)

## Use

| Action | How |
| --- | --- |
| Add a project | `+ Add project` at the left of the bar along the bottom, or Ctrl+Shift+O, lists the git repositories found below your home directory, most recent first. Type to filter, Enter or a click adds one. `Browse folders…`, or a text starting with `/` or `~`, lists folders instead: a click or Enter goes into one, the first row adds the folder you are in. A second click on `+ Add project` closes the list. Or run `codn FOLDER` |
| Remove a project | The cross beside its name, once it has no sessions |
| Start a session | The plus beside the project, or Ctrl+Shift+E, opens a list: the agents, the one you used last in this project first. Enter or a click starts one. Ctrl+Shift+T starts that first one at once |
| Set the model and the like | In that list, the arrow at the right of an agent or the Right key shows what can be set: Left and Right change a value, Enter starts. What you set is remembered per project and agent and shown in the list |
| Continue an earlier conversation | The same list shows, below the agents, the five most recent conversations they have on disk for this folder, and a row that shows the rest. Typing searches all of them, Enter or a click opens one as a session |
| Open a session beside the current one | Shift+click it. The two columns among the icons at the top right of the window put another session of the project beside it, or start one with the same agent |
| Switch between the two panes | Ctrl+Shift+Left, Ctrl+Shift+Right |
| Close the second pane | The cross beside its title, or Ctrl+Shift+W. The session keeps running |
| Rename a session | Double click it, or Ctrl+Shift+R. Enter confirms, an empty name resets it |
| Show what changed | Click the branch name of the project or the page among the icons at the top right of the window, or Ctrl+Shift+D |
| Fold the sidebar | The first of the icons at the top right of the window, or Ctrl+Shift+B, folds it to a strip and brings it back. The strip keeps of every session the mark of what it is doing, and a click opens the session. A list, the field that renames a session and a message are shown in the whole sidebar, over the pane, for as long as they are there |
| Show the files of a project | The icon left of the buttons of the window, or Ctrl+Shift+F, shows a column at the right and folds it away. It shows the project you are working in; a click on the row of another project shows that one. A click opens a folder |
| Read a file | Click it in that column. It is shown where the changes are shown, in the colors of its language where codn has a grammar for it. Arrows, Home, End, PageUp and PageDown move through it, with Ctrl by words and to its ends, with Shift selecting. Shift with the wheel, or scrolling sideways, moves lines that are longer than the pane into view. Ctrl+A selects all, Ctrl+C copies, Escape closes it. Ctrl+Shift+Left and Right go to the other pane, as everywhere |
| Change a file | Type into it. Ctrl+S saves, Ctrl+Z takes a step back, Ctrl+Shift+Z makes it again, Ctrl+X, Ctrl+C and Ctrl+V cut, copy and paste. The name of a file with changes that are not saved has another color, and before such a file gives way the lines at its bottom ask: Ctrl+S saves and goes on, Ctrl+Shift+Backspace lets the changes go, Escape stays. A file that another program wrote to meanwhile is not written over: Ctrl+R takes what is on the disk, Ctrl+Shift+S writes what is in the pane |
| Read Markdown as it is meant | A file of Markdown opens with its headings, lists, tables and code blocks drawn, and its text wrapped at the width of the pane. The icon beside the cross of its pane, or Ctrl+Shift+M, shows its text, where it is changed, and goes back. The wheel, the arrows, PageUp, PageDown, Home and End move it. Its links are told by their color and lead nowhere yet |
| Look at a picture | A PNG or a JPEG in that column is shown as a picture, fitted into the pane. Ctrl with the wheel zooms it about the pointer, `+` and `-` zoom it too, `0` fits it again, and a double click goes to its own size and back. Held by the button it is moved, and by the wheel and the arrows. The lines under it say its size and how far it is zoomed |
| See what changed in a file | Lines that are new or changed since the last commit have a green surface, and where lines went the number of the next line is red. Click such a number for the changes at that place, and the name of a file in the changes for the file |
| Next and previous session | Ctrl+Tab, Ctrl+Shift+Tab |
| Copy and paste | Ctrl+Shift+C, Ctrl+Shift+V, middle click pastes the selection |
| Scroll back | Mouse wheel, Shift+PageUp, Shift+PageDown |
| Open an address from a terminal | Hold Ctrl: the address under the pointer is underlined, and a click opens it in your browser. An address is what begins with `http://` or `https://`. Ctrl+click beside one does what it did before |
| Font size | Ctrl+Plus, Ctrl+Minus, Ctrl+0 |
| Move and maximize the window | Drag or double click the top strip. On Wayland a right click opens the window menu |

Shortcuts are tied to key positions where a layout has no Latin letter on
the key, so Ctrl+Shift+C is the same key on a Russian or Greek layout.

A session is called what its program reports as the terminal title, Claude
Code for example reports the topic of the conversation. Sessions that would
read the same are numbered, and a name you give wins over both. Which
program a session runs is said by its mark before the title, in the header
of its pane and in the list a session is started from.

The mark in front of a session shows what it is doing, by its shape and by
its color: a gray ring at rest, a yellow glider that moves over a small
field while it works, and a blue dot when it stopped while you were not
looking. That much is derived from terminal activity, so it works for any
program without setup.

A program can also say itself what it is doing, with the program status
protocol (OSC 7501), and is then believed before its output. Then there is
a blue question mark while it waits for you, whatever it prints, a green
check when it is done, until you looked, and a red triangle when it says it
failed. The notification says what it waits for. Claude Code speaks the
protocol from 2.1.295 on, to a terminal that answers its question, which
codn does. It does not while `CLAUDE_CODE_DISABLE_TERMINAL_TITLE` is set.

What a program says it runs beside itself has rows below its session,
each with an arrow that leads to it from what it hangs on: the agents a
session of Claude Code sends off, each with the mark of its state and
the few words it was sent off with, or what it is doing now where the
program says so in the file below (`agents`), which the plugin for
Claude Code does. Such a row goes when its agent has ended, and a click
on one opens the session. Eight of them are shown, and a last row says
how many more there are. In the protocol these are the records that have
an `id`, so any program can have such rows. Codex says nothing of its
agents to a terminal, and has none.

The red triangle also means that the program is not running: it could not
be started, or it ended with an error. The session stays in the list with
what it printed last, and Enter starts it again. Only a program that ends
cleanly takes its session with it, as a shell does after `exit`. A shell
ends with what its last command ended with: after one that failed, its
session stays.

### The bar along the bottom

Along the bottom edge of the window runs a bar. At its left is what adds a
project. Beside that it shows what the program of the session in front
reports about itself: the model and what it runs with, such as its effort,
and at the right one ring for the context and one for each limit, filled
as far as it is used, and before the rings how fast the model answered
last, in tokens a second. Where a program reports neither model nor options,
the bar shows what you chose before the start, unless that was the first
choice of every option. For a plain shell it shows nothing.

codn names a file to every program it starts, in the variable
`CODN_STATUS`. Whatever writes these keys there, as TOML, is shown, and
any of them can be left out:

```toml
model = "opus"       # shown at the left
options = ["high", "think"]  # what it runs with, in its own words, at most four
tokens = 124000      # in the context now
window = 200000      # what the context holds
speed = 78           # tokens a second, as the model answered last

[limits.5h]          # by the name to show, at most four
used = 34            # percent
resets = 1791315360  # when it is whole again, in seconds since 1970

[limits.7d]
used = 12.5

[agents]             # last in the file: what the agents are doing, by their ids
a89b5d4516757b675 = "Read sidebar.rs"
```

That reads `opus · high · think` at the left and, at the right,
`78 tok/s`, `62% context · 124k / 200k`, `34% 5h · resets 19:36` (where the
clock shows Greenwich time: a reset is shown in your own) and `12% 7d`. A
limit can also be a number alone, and a line of shell does as well as an
agent: `printf 'model = "build"\n' > "$CODN_STATUS"`.

- A reset is shown as the time of day it happens, with the day of the week
  if that is a day away or more, not as the time that is left.
- Every figure has the room of `100%`, so a ring stays where it is when one
  grows a digit. In a narrow window the rings give up what stands beside
  them, first the detail and then the name, and where not even the
  figures fit, the speed goes and then the last rings. Once the speed
  is gone, the rings say again what fits without it.
- The speed is shown as it is written, and stays until another is
  written: codn does not know when a program rests.
- The file is read a second after the session printed something or took a
  key, at most once a second while it does either, and once more if the
  last of it was less than half a second before a read.
- A file that is empty or does not parse leaves what is shown: its program
  may be writing it. A file that is gone clears the bar.
- What stands under `agents` is shown in the rows below a session, not in
  the bar: the row of an agent says it in place of the few words the
  agent was sent off with. The key is the `id` the program reports that
  agent under to the terminal (OSC 7501), and a key no row has is shown
  nowhere. At most 32, each at most 48 characters. They stand last in
  the file, behind the line `[agents]`, one agent to a line: what
  follows another table there is not read, and a report that has a
  text of several lines before that line has no agents.

Claude Code writes the file through the plugin in `mods/codn-status`, which
codn does not load for you. Start `claude` with it once to try it, or name
the folder for every session in `~/.claude/settings.json`:

```sh
claude --plugin-dir /path/to/codn-app/mods/codn-status
```

```json
{ "env": { "CLAUDE_CODE_PLUGIN_DIRS": "/path/to/codn-app/mods/codn-status" } }
```

For every agent the session starts it reports the tool that agent called
last, in a few words made of the call: for a command the words Claude
writes of it (`Show working tree status`), else the tool and what it was
called on (`Read sidebar.rs`, `Grep OpenBelow`). It is written as the
tool begins and taken out when the agent has ended. The command itself
is never written, nor the name and password an address can carry, but
the name of a file, a pattern and what is searched for are: the file
lies in codn's folder for its state, which only you can read, until
codn is started next.

For the options it reports the effort and, unless thinking is switched
off, `think`. The effort is that of the settings until the first turn has
ended, and from then on the one the turn ran with, which also knows an
effort the session was started with.

For the speed it reports the tokens of the last answer of the model over
the time that answer took, from its first piece to its last: the wait
for the first piece is not counted, nor the time the API ran a tool of
its own inside the answer, to the end of the answer where the result of
that tool did not arrive. A turn is many answers, one before every
call of a tool, and the speed is that of the last of them. An answer
shorter than a second says none and leaves the one before, which also
stays while the session rests and after `/clear`. What it thinks is
part of an answer and counted. The answers to the agents a session
sends off are not counted: several arrive at a time. A request that
failed half way and is sent again counts from its first attempt, which
makes it slow: the plugin is not told that an answer began anew. Where
another model answered than the one the bar names, the speed is that of
the one that answered.

Outside codn the variable is not set and the plugin writes nothing. Nor
does it for a `claude -p`: such a run, started by something in a session,
inherits the session's file and would put its own figures over those of
the program codn started. The plugin is written against an API of Claude
Code that is in early access and may change from one release to the next.

Codex writes the file through `mods/codn-codex/report.py`, which Codex
starts as its `notify` whenever a turn has ended, and which codn does not
bind for you. Name it in the arguments of the agent in codn's
configuration, with the file of the start behind it:

```toml
args = ["-c", 'notify=["/path/to/codn-app/mods/codn-codex/report.py", "{status}"]']
```

It needs Python 3 and nothing besides. It reports the model and its
effort, what the context held at the last call of the model, and the
limits of the account under the length of their windows, `5h` and `7d`.
It reads them from the transcript Codex keeps of the session, which is
Codex's own format: what it no longer finds there is left out. It
reports no speed: Codex starts it when a turn has ended, and it does not
see an answer arrive.

- The file is an argument and not read from `CODN_STATUS`, because Codex
  can run its turns in a background server that all its sessions share,
  and that server has the environment of the session that started it. An
  argument of `-c` also makes Codex run without that server, which it
  says once under its warnings.
- The bar changes when a turn ends. After a turn you interrupted, after
  `/compact` and after choosing another model it shows what it showed,
  until the next turn has ended. Before the first turn it shows what you
  chose before the start.
- A `codex exec` writes nothing, nor does an agent that Codex sends off.
  A `codex` typed into a shell has no reporter and shows nothing.
- A `notify` of your own in `~/.codex/config.toml` does not run in the
  sessions codn starts this way: the argument takes its place.

Closing codn ends the running programs: HUP, then TERM after two seconds,
then KILL after another two seconds for processes still in the owned PTY
session. On the next start the sessions are back, and each one is started
again when you first open it. Claude Code continues its known conversation,
also after `/clear` or `/resume` changed it and its running metadata was
read. Codex conversations chosen from the history continue by their explicit
id. A newly started Codex session starts fresh after an app restart, since
Codex exposes no running conversation id; codn does not infer one from files
that another process might have created. Starting codn
while it is running opens no second window: the folders you name are added
to the one that runs.

## Configure

Optional, in `~/.config/codn/config.toml`. A mistake in the file is shown in
the sidebar, and the defaults are used. Font size must be finite and between
8 and 40 logical pixels; scrollback may be at most 1,000,000 lines.
The configuration file is limited to 1 MiB.

```toml
scrollback = 10000
roots = ["~"]               # where "+ Add project" looks for git repositories

[font]
family = "JetBrains Mono"   # default: the system monospace font
ui_family = "Inter"         # default: the system sans-serif font
size = 14

[window]
buttons = true              # false hides minimize, maximize and close

# Naming agents says which there are and in which order. One that is not
# named is not offered, nor is one whose command is not installed.
# An agent named claude, codex or shell is the built-in one of that name:
# it keeps what you do not say, and what you say takes the place of what
# is built in. Any other agent has nothing but what you say.
[[agent]]
name = "claude"                    # as it is built in

[[agent]]
name = "codex"
# What fills the bar along the bottom for Codex, see there.
args = ["-c", 'notify=["/path/to/codn-app/mods/codn-codex/report.py", "{status}"]']

[[agent]]
name = "shell"
cmd = "/bin/zsh"                   # built in: your login shell

# An agent of your own, with every key there is. The values are those
# built in for claude.
[[agent]]
name = "other"
cmd = "claude"
icon = "agent"                     # the drawing shown before the title of its sessions
mark = "✳"                         # one or two characters, if there is no icon
args = []                          # arguments for every start
start = ["--session-id", "{id}"]   # extra arguments for a new session
resume = ["--resume", "{id}"]      # extra arguments after a restart
shift_enter = true                 # Shift+Enter inserts a line break

[agent.history]                    # where the agent keeps its conversations
files = "~/.claude/projects/{folder}/*.jsonl"
id = "sessionId"
folder = "cwd"                     # also checks collisions in encoded folder names
title = ["customTitle", "aiTitle", "lastPrompt"]
running = "~/.claude/sessions/{pid}.json"

[[agent.option]]                   # what can be set before a session starts
name = "Model"
choices = [
    { name = "default" },
    { name = "opus", args = ["--model", "opus"] },
    { name = "sonnet", args = ["--model", "sonnet"] },
]

[[agent.option]]
name = "Effort"
choices = [{ name = "default" }, { name = "high", args = ["--effort", "high"] }]
```

What an agent says takes the place of what is built in as a whole: one
`[[agent.option]]` and the options are those of the file, an
`[agent.history]` and the history is. What it does not say it gets from
the codn you run, so a model that is built in later is in its list.

`icon` is one of sparkle, code, terminal, agent, robot, wand, star, chip,
lightbulb and beaker: drawings that say what kind of program an agent is.
Claude Code and Codex have the agent, a shell the terminal.

`{id}` is a UUID codn generates per session. `{status}` is the file codn
names to this start of the program, the one in `CODN_STATUS`, for an agent
whose reporter does not run with the environment of its session; it is put
in as it is, and is empty where there is no file. `shift_enter` sends
Shift+Enter as Alt+Enter to programs that do not use the kitty keyboard
protocol, which agent CLIs read as "new line". Leave it off for shells and
editors.

`[[agent.option]]` is something to choose before a session starts. Each
choice has a name and the arguments that make the agent do it. The first
choice applies until another is chosen, so one without arguments leaves
the matter to the agent's own configuration. The built-in agents offer:
for claude the model, the effort and the permission mode, for codex the
model, the reasoning effort and the sandbox, as the two programs had them in
October 2026. A model that came out since is one more choice in an option
of your own. What you change inside a running session does not change
what codn starts it with the next time. The bar along the bottom shows a
model that the program reports, also one you changed to.

`[agent.history]` tells codn how to find an agent's conversations, in files
of JSON lines that the agent writes for itself. Nothing about any agent is
built into the program beyond these defaults, so when an agent changes its
files, or for another agent, the description is all there is to adapt:

| Key | Meaning |
| --- | --- |
| `files` | The files, one per conversation. `*` stands for any part of a name, `{folder}` for the project folder with every character that is not a letter or digit replaced by `-` |
| `id` | The key whose value is the id the agent resumes by. It goes where `resume` says `{id}` |
| `folder` | The key whose value is the folder a conversation ran in. An exact folder check also prevents collisions when `{folder}` encodes different paths alike; leave it out only if the file layout is unambiguous |
| `title` | Keys that hold a title, the preferred one first. A conversation without a title is not listed |
| `index` | A file with one line per conversation that is searched for titles first |
| `only` | Keys and the values one of which a conversation must have. The default for Codex leaves out `codex exec` runs and subagents |
| `running` | A file the running agent keeps about itself, `{pid}` being its process id, which names under `id` the conversation it is in |

Keys are found at any depth of a line. With `running`, codn follows an agent
that goes on to another conversation. Without it, as for Codex, only ids
explicitly selected from history are known. No id is adopted from file
creation times. Changes inside Codex, such as `/new` or `/resume`, cannot
be followed; an explicitly resumed session will still resume its original
id after an app restart. History reads are bounded and skip nonregular
files; incomplete or unsafe ids are ignored.

## What codn runs on its own

codn asks git for the state of your projects without being told to: when a
project is added, when the window gets the focus, and every few seconds
while an agent works. A repository can name programs for git to run in its
own configuration, so these calls switch all of that off: file system
monitors, hooks, filter and diff drivers defined by the repository, lazy
fetches. Worktree configuration is included. Dirty files inside submodules
are ignored by status; changed submodule commits remain visible. Drivers
from your own git configuration, such as git-lfs, stay. A repository that
names another folder as its work tree is not read at all, unless that
folder names it back as a submodule's does.

Where an agent that is named is not found on the search path codn was
started with, as with a launcher of a desktop, codn asks your login
shell for its search path, once at its start: it runs `$SHELL -ilc`
with a line that prints the path, which runs the files your shell
reads when you log in. It waits ten seconds for the answer at the
most and takes of it the path alone.

Programs in a terminal can put text on the clipboard, but only in a session
you are looking at, and they can never read it.

An address a program printed is opened only by your click on it with Ctrl
held, never by the program: codn then starts `xdg-open` with that address
and nothing else. Only what begins with `http://` or `https://` is one,
of 2048 bytes at the most, and it is the text that is underlined: text a
program hides, or prints in the color it stands on, is not part of it,
and while it is underlined it is drawn in codn's own colors, letters,
ground and line, whatever colors the program gave it. A file, or the
scheme of some application, is not handed on.

codn starts itself a second time, as `codn --syntax`, when a file is shown
that it has a grammar for: nineteen languages, by the ending of the name.
That process finds the colors of the text and nothing else. It gets the
text over a pipe, has no file of yours open and none of the app's, may
use 1 GiB of memory at the most, and ends when the file is closed or
codn ends. The grammars and the library that reads them are other
people's work, and a file can be made that keeps such a library busy
without end or ends it. Where one answer of that process, a stretch of
lines, takes longer than eight seconds, the process is ended, the file
is shown in one color and says so, and nothing else is touched. A file
of many lines that each take less keeps one core busy for as long as
it is shown. When the file was written to it gets one more try. A
file over 1 MiB is not sent to it, and behind a line over 16 KiB a file
has one color.

The same process reads a file of Markdown that is shown as it is meant,
with a second library of other people's, `pulldown-cmark`. It says what
the text is made of, and codn draws from that. Where it ends over a
file or says what does not fit the text, the file is shown as its text
and says so.

For a picture codn starts itself once more, as `codn --picture`: one
process for one PNG or JPEG, which is handed the file as its input, says
once what it made of it and ends. Before it reads a byte it closes what
it inherited and sets itself limits: 256 MiB of memory, ten seconds of
a core, no file open beside the three it was started with, and none
that it could make longer. Without those limits it does not run. They
bound a decoder that fails, loops or asks for memory without end; they
are no cage for one that a file made run what the file brought. The
decoders are codn's own and written without
`unsafe`; what unpacks a PNG is `yazi`, other people's work. A file over
64 MiB or with more than 134 million pixels is not shown, and where the
process fails or is ended the pane says so and nothing else is touched.

codn reads the file it named to a program in `CODN_STATUS`, which lies in
`~/.local/state/codn/status/`: one for every start of a program, so that
nothing an earlier one left or still writes is taken for the next one's.
What earlier runs of codn left there is removed at the start. Of a file
only numbers, two kinds of names and times of day are shown, each within
bounds, and so are the options, which are names. At most 16 KiB are read,
and no file that could nest deeper than 64 levels: a small one can go
deep enough to end the thread that reads it. A name keeps 48 characters,
none of them a control character or one that is not seen. A part is at
most 999 percent, a count of tokens at most 999 million, a speed at most
99,999 tokens a second, and a reset lies ahead by a hundred years at
most.

Clipboard reads run on a worker and expire when the owner does not reply:
a paste that brings nothing for that, or because the clipboard holds
more than is taken, says so in the sidebar.
What is typed after a paste waits for the pasted text.
Clipboard text and queued PTY input are limited to 8 MiB. Oversized input
is rejected as a whole with a notice, preserving previously queued input.
Copies are limited to 100,000 cells and 8 MiB. Terminal OSC payloads are
limited to 8 MiB and combining marks to 128 per cell.

There is room for a thousand projects, and four thousand sessions are
kept from one start to the next.
State is written atomically with private permissions, limited to 16 MiB;
state that cannot be read is preserved as `state.toml.bad`, a later one
beside it with a number. Problems are written on a
bounded worker to `~/.local/state/codn/codn.log`, rotated at 1 MiB.

## The code

Where what lives:

```
src/core      projects, sessions, status, git, config, saved state, the
              files of a project and the text of one, what programs report
              about themselves (no drawing, no window system)
src/term      PTY and terminal emulator, key and mouse encoding, the grid
src/syntax    the colors of a file and what a file of Markdown is made
              of, found by a second process of codn
src/picture   a PNG or a JPEG made into pixels, by a process for each
src/render    fonts, glyph atlas, the GPU pipeline
src/ui        sidebar, panes, diff view, file tree, file view, the
              preview of Markdown, project picker, the bar, window
              chrome, theme
src/platform  window, clipboard, notifications, single instance, what
              opens an address
src/app       the event loop: keyboard, pointer, sessions, files
src/action.rs what clicks and shortcuts can ask for
mods/         the plugin through which Claude Code reports to codn, and
              the reporter for Codex
scripts/      what measured the numbers in the README, and what compares
              the pictures of two builds
```

A change is checked with `cargo fmt --check && cargo clippy --all-targets
&& cargo test`.

Adding something clickable means: a variant in `Action`, drawing it with its
hit, and handling it in `App::run`.
