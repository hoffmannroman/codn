# Changelog

What changed in codn from one version to the next, written for the people
who use it. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
the version numbers follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
The first version that is published is 0.3.0; the ones before it are
here for what changed in them.

## [Unreleased]

### Changed

- **The list that starts a session shows the five most recent
  conversations** of the project below the agents, not all of them at
  once, and a row that says how many more there are. A click or Enter
  on that row shows them all; the next time the list opens it is short
  again. Typing searches every conversation, as before.

## [0.3.0]

A file is shown as what it is: a PNG and a JPEG as pictures that can be
zoomed and moved, and a file of Markdown as it is meant, with its
tables. An address in a terminal is opened by a click. The agents a
session sends off stand below it in the sidebar, each with what it is
doing, and the bar says how fast the model answered last. The colors
are built anew, and calmer, and the mark of a session at work is a
glider.

Ctrl+Shift+M shows a file of Markdown as its text and goes back.
Control with a click opens the address under the pointer.

### Added

- **A PNG and a JPEG are shown as pictures**, in the pane where a file
  is shown, fitted into it and never larger than it is: one pixel of
  it is one of the screen at the most. A picture is zoomed with Control
  and the wheel, about the pointer, up to eight pixels of the screen
  for one of it, and with `+` and `-`; `0` fits it again, and a double
  click goes to its own size and back. Held by the button it is moved,
  and by the wheel and the arrows. The line under it says how far it is
  zoomed. A JPEG is turned upright as its camera says. Of a JPEG the
  plain kind is shown; a progressive one, one in CMYK and one coded
  arithmetically or with twelve bits say so and show their name and
  size. What of a picture is transparent lies on a grey
  chequer, on which a drawing in black is seen and one in white. The
  lines under it say its size, and what there is to say of it: that the
  file ends early or is damaged, of which what could be read is shown,
  that it holds an animation, which is not played, that it says how its
  colors are meant, which is not applied, or that it is too large to be
  shown any finer. A file that is not shown says why: one packed as
  Apple's tools do it, one over 64 MiB, one with more than 134 million
  pixels. A picture an agent is writing stays as it was until the file
  is whole. Beside a session that has the keyboard, a picture is not
  dimmed as other panes are, only the lines under it. The file is
  decoded in a process of its own, which can open nothing and has
  limits on its memory and its time, so that no picture can end codn
  or a session. Other pictures show their name and size as before, an
  SVG its text.
- **A file of Markdown is shown as it is meant**, in the pane where a
  file is shown: headings, flowing text that is wrapped at the width of
  the pane, bold, italic and struck through, lists with their signs,
  their numbers and the boxes of tasks, quotes, rules, tables, and code
  blocks in the colors of their language. The icon beside the cross of
  the pane, or Ctrl+Shift+M, shows its text and goes back, and the view
  stays at the line it stood at. Nothing is typed into the file as it
  is meant: the wheel, the arrows, PageUp, PageDown, Home and End move
  it, and changing it is done in its text. A table that is wider than
  the pane has its cells wrapped. What is not drawn is shown as it is
  written: HTML, the head of a file between dashes, the mark of a
  footnote, and a table for which the pane is too narrow. A comment of
  HTML is left out. A link is told by its color and leads nowhere yet,
  and where a picture stands there is a sign and its text. A file an
  agent is writing is shown anew as it changes, at the place it stood.
  A file over 1 MiB, one that is not UTF-8 and one with more to it
  than a preview is made of are shown as their text, and the lines at
  the bottom say why. Markdown is read by `pulldown-cmark`, in the
  process that finds the colors of a file, so that no file can end
  codn or a session.
- **An address in a terminal is opened by a click with Control held.**
  While Control is held, the address under the pointer is underlined
  and the pointer is a hand; a click then opens it in the browser of
  the desktop, through `xdg-open`. An address is what begins with
  `http://` or `https://`, up to the next blank and without what closes
  a sentence or a bracket behind it. One that the terminal broke
  because the row was full is one address. A click without Control
  selects as before, and so does Control with a click beside an
  address; where a program tracks the mouse, the address is opened and
  a click beside it goes to the program. A file and other schemes are
  not opened, nor is text that a program hides or prints in the color
  it stands on.
- **The agents a session sends off, as rows below it in the sidebar.**
  Each has the mark of its state and the few words it was sent off with,
  and goes when it has ended. A click on one opens the session. Eight
  are shown, and a row below them says how many more there are; the
  strip of a folded sidebar keeps the mark of the session alone. They
  are what a program reports with an `id` in the program status protocol
  (OSC 7501), which Claude Code does for the agents it starts. Codex
  reports none.
- **Such a row says what its agent is doing, and an arrow leads to it
  from its session.** A program can say in the file codn names it in
  `CODN_STATUS` what each of its agents is doing, under `agents` and by
  the id it reports the agent under; the row shows that in place of the
  words the agent was sent off with. The plugin for Claude Code,
  `mods/codn-status`, writes there the tool an agent called last: the
  words Claude writes of a command, or the tool and its file. Those
  lines stand last in the file and are read one by one, so that they
  may name files without the dots of all of them counting as depth.
  Before each row stands an arrow that comes from above and turns to
  it, below the mark of its session, or of the row it lies below.
- **How fast the model answers, in the bar along the bottom.** Before
  the ring of the context stands the speed of the last answer, in
  tokens a second: `78 tok/s`. The plugin for Claude Code,
  `mods/codn-status`, says it after every answer of the model, of which
  a turn has one before every call of a tool: the tokens of the answer
  over the time from its first piece to its last, without the wait for
  the first. An answer shorter than a second says none. The speed stays
  while the session rests, and it is the first to go where the window
  is too narrow for the figures. Any program can say one, under `speed`
  in the file codn names it. Codex says none.

### Changed

- **The colors are built, and calmer.** Six hues, 60 degrees apart, and
  every one of them equally light and equally colorful, so that no
  keyword is louder than a string and no type lighter than both: the
  colors of a text, of the marks of a session, of the changes and those
  a program names by number in the terminal are the same six. They are
  a fifth less colorful than before, for eyes that look at them for
  hours. A number in a text is cyan where it was orange, and
  the rings of the bar have that cyan. What warns stays louder than the
  rest. Faint text, as hints and the numbers of lines in the changes
  are, is lighter and easier to read, and so are the hints programs
  write in bright black. Text that a program dims is dimmed by as much
  as the eye takes it, whatever its color. The grays are reckoned too,
  and none of them moved by what can be seen.
- **The mark of a session at work is a glider.** Where an arc went
  round, a glider of the Game of Life moves over a small field of five
  cells by five and comes in again at the other side, five steps a
  second. It is not a film that is played: every step is reckoned by
  the rules of the game. It draws five frames a second where the arc
  drew eight.

### Security

- The browser that a click on an address opens, and what shows a note
  on the desktop, hold none of the files codn has open. They inherited
  what a library had left open, memory of the graphics driver among it.
- A file that codn reads cannot become the terminal of codn. A program
  could make the file of its report a link to a terminal it held, and
  codn was ended, with every session, when the program let go of it.
- What earlier runs left of reports is removed only from a folder of
  codn's own, not from one that a link leads to.
- A program that asks its terminal for a color, or for the size of the
  window, without end cannot fill the memory of codn any more: such a
  question was kept for the window, faster than it was taken, and
  three gigabytes were full in seconds. So was text that a program put
  on the clipboard again and again.
- A hook that a repository names in its configuration does not run
  when codn asks git about it. Hooks in the folder of a repository
  were switched off, these were not, and the one that follows a change
  of the index ran when the changes or a file of the project were
  shown. codn's questions to git write the index no more.
- The changes of a project show no file outside its repository, and no
  row of them leads to one: an index can be written that names such
  files, and a click on the row opened that file to be changed. Nor
  does a row lead out of the folder of a project that lies inside
  its repository: a name with two slashes behind that folder was a
  path from the root once it was cut there, and no path that leaves
  the folder of its project is opened in a pane, wherever it comes
  from.
- A save writes where the name of the file led when it was read, and
  nowhere else. A link that a program put under the name since, to a
  file anywhere on the disk, was written through: without a word where
  that file held the same bytes, and on Ctrl+Shift+S where the line
  spoke of another text.
- A click with Control opens the address that is underlined, and no
  other: it asked the terminal anew, and a program that printed
  another address over the first while the terminal was busy had its
  own one opened.
- An address is drawn in codn's own colors while it is underlined,
  letters, ground and line. A part of it that a program printed in
  colors near each other could not be read, and the line under it not
  be seen, and it was opened with the rest.
- A program in a session inherits no file that codn has open: the
  drivers of graphics left memory of theirs open to it.
- What the process for a picture says of the size of the picture and
  of the part it shows is held against what a picture may have and
  against the part that was asked for.

### Fixed

- A file of text with a zero byte far behind its start is shown and
  not changed, as one with a zero byte near its start is no text at
  all. It could be changed, and once a key had taken that byte out,
  Ctrl+Z was refused, for that step and for every one before it.
- Ctrl+R said a second time, where what the disk holds cannot be
  taken back, takes it also where the disk holds by then the bytes
  the file was read from: what was typed stayed, and was called
  saved.
- Ctrl+R said while a save was being written takes nothing after that
  save: the next time the file was read, however much later, it was
  taken over what was typed since. And a file that codn itself is
  saving is not called changed on the disk until the save has
  answered.
- A file of Markdown that is too long to be shown as it is meant
  stays its text while it is changed. A key that made it short
  enough gave it to its preview, where nothing is typed, with the
  changes that were not saved. The icon and Ctrl+Shift+M ask for it
  as it is meant.
- The view of a file of Markdown as it is meant stays where it was
  inside a paragraph that is written in one line, when the file is
  read again, when the pane is made wider and when its text is
  switched to and back: it went to the beginning of the paragraph.
  PageUp passes over no rows where those above are higher than those
  below, and after a pane was made wider the view does not stand
  below where the last rows just fit.
- In a file of Markdown as it is meant, pictures that stand side by
  side go on in the next row where the pane ends: their signs stood
  in one row, and those behind the right edge were not seen.
- A file of Markdown with very many marks that go onto the sign before
  them, hundreds of thousands in one row, does not slow every frame:
  each of them was drawn, 77 ms a frame where it is 2 ms now.
- A file of Markdown with very many signs that differ is set in its
  rows six times as fast, while the window is dragged wider or
  narrower too: an image was made of every sign to learn its width.
- A file that grows over 1 MiB while it is typed in, or by what is
  taken from the disk, has one color from then on, as one that is
  read so large: the line that was typed into kept the colors of the
  line it had been, and nothing took them away.
- A program that ends with an error in a session you are not looking
  at is told by a note, "Failed", as one that says so itself.
- Of two sessions side by side, the one with the keyboard stays when
  the changes of a project are opened, and keeps the keyboard, as
  when a file is opened: the changes took its place.
- A session whose program ends with an error while it is at work is
  not at work any more: its mark went on, and a moment later a note
  said that the session waits for you.
- A state with very many sessions that are not kept, of folders that
  are no projects any more or under ids that are there twice, does
  not keep the window away at the start: 289,112 such rows, in a
  state smaller than is read, took ten seconds.
- The plugin for Claude Code holds no call of a tool: an agent that
  fetched an address whose machine was long and no name of one waited
  for about a second before the fetch began, in every session that
  loads the plugin, also outside codn.
- The plugin for Claude Code does not make the start of a session and
  the end of a turn wait for the disk, and writes its file only when
  there is something new to say: the end of a turn wrote it three
  times.
- The plugin for Claude Code writes a name no longer than codn reads,
  and no more limits than codn shows. A model or a limit with a very
  long name emptied the bar, and three single quotes in a row in a
  name took the rows of the agents their words.
- The reporter for Codex passes over a line of a transcript that is
  longer than 4 MiB: it took three times the length of its longest
  line in memory.
- The command of an agent may begin with `~/`, as the folders of the
  configuration may.
- codn starts also where the folder of its state has a long path and
  there is no folder for what runs (`XDG_RUNTIME_DIR`): the socket of
  the one instance could not be made there, and codn ended with a word
  in its log alone.
- A folder that cannot be a project is said so in codn's own words,
  with its name short and on one line.
- A mistake of the configuration is said in one short line, whatever
  value it was made with.
- A line of the log is one line: a line break or another character
  that controls a terminal, as the name of a file can hold it, is
  written as its name, and read as a record of its own before. The log
  is written also where nobody reads what codn says to its terminal.
- A state of many projects does not keep the window away at the start:
  there is room for a thousand projects and four thousand sessions,
  and forty thousand projects, which half a megabyte of state holds,
  took most of a minute before the first frame, at every start.
- Text that is changed quickly while its colors take long does not
  pile up in memory: every state of it waited whole, a megabyte at a
  time, until the colors of the one before were found.
- A file that went and is made anew by a save has the permissions it
  had, not those the system gives a new file: a file for the user alone
  became one that others may read.
- A new file whose name is a dash is shown among the changes with its
  lines, and one whose name is no text is named in a note: the first
  had no lines, the second was left out without a word.
- The changes of a project are read rightly also where its repository
  sets `diff.suppressBlankEmpty` or `diff.relative`: the numbers of
  lines were off behind an empty line, and files were named from the
  folder asked in.
- Copying text whose row begins with the second half of a wide letter
  keeps that row.
- A report of a program's state (OSC 7501) whose `id` is none is passed
  over whole. It was taken for a report of the program itself.
- A name and a title that a program reports keep no character that
  draws nothing: fillers, the empty pattern of Braille, tags.
- In the bar: limits whose names read alike take one place and no other
  limit's, a ring shows the whole percent that its figure says, nothing
  used is never shown as `-0%`, and a count of a hundred million tokens
  and more has no tenth.
- A window that another program takes away ends codn as closing it
  does, with its state written. On X11 codn then ended with an error,
  and every session with it, or ran on without a window: every later
  start handed its folders to it and showed nothing.
- On X11 a long text is pasted from a program that hands it over in
  pieces: above a quarter of a megabyte nothing came, without a word.
  And a paste that brings nothing because the clipboard did not answer
  in time, or holds more than 8 MiB, says so in the sidebar.
- `CODN_TIMING` does not end codn when the terminal that took its
  lines is closed, and does not make the window wait where nobody
  reads them.
- While the lines at the bottom of a file ask about changes that are
  not saved, what an input method makes of keys does not go to the
  session beside the file.
- A session does not take itself for running in Zellij or in Konsole
  when codn was started from one of them.
- A terminal has 2048 columns and 1024 rows at the most, and what one
  frame of a very large window made codn hold is given back. A window
  that another program made as large as the window system allows had
  a terminal of 6347 columns and 2902 rows: two gigabytes, which
  stayed when the window was small again.
- A font file that was cut off is not taken as the font. It was, and
  nothing was drawn anywhere: no sign had a size.
- A font that is written anew in place while codn runs, as `cp` does
  it, does not end codn any more, with every session: its file was
  mapped, and is read now.
- The sidebar costs what is seen of it. With a thousand projects every
  frame drew the rows of all of them, 4 ms, and with three thousand
  sessions in one project a frame took 27 ms: both take 0.3 ms.
- A click on a row of the list that adds a project or starts a session
  chooses nothing when the list was built anew since it was drawn: by
  then the row under the pointer could be another one.
- A line of the changes draws sixteen marks onto one sign at the most:
  a line of nothing but marks drew a thousand.
- A scale of the window that no screen has, from a setting or a
  variable, is taken as a quarter at the least and sixteen at the
  most, and no image of a sign or an icon is made at a size that is
  no number.

## 0.2.0

The files of a project: a column that lists them, and a pane that shows
a file in the colors of its language, with what changed since the last
commit, and lets it be changed and saved. What a session is doing, said
by its program where it says so, as a mark of its own shape. A sidebar
that folds to a strip, and one place at the top right of the window for
the icons of what is shown beside the sessions.

Ctrl+Shift+B folds the sidebar. The column with the files, which had
that key while it was unreleased, is on Ctrl+Shift+F.

### Added

- **The files of a project, in a column at the right.** The icon left of
  the buttons of the window, or Ctrl+Shift+F, shows it and folds it away.
  It lists every file and folder but `.git`, hidden ones and what git
  ignores among them, folders first. A click opens and closes a folder.
  Before a file stands an icon for the kind of file its name tells.
  The column shows the project you are working in, and a click on the row
  of a project in the sidebar shows that one. It reads the folders again
  when the window gets the focus, when a session went quiet, and while an
  agent works. Where the window is too narrow for it, it gives way and
  comes back with the width.
- **A file, shown in a pane.** A click on a file in that column shows its
  text where the changes are shown, with line numbers. The arrows, Home,
  End and the page keys move a caret through it, with Control by words
  and to the ends of the text. Dragging, a double and a triple click and
  Shift with those keys select, Ctrl+A selects all, Ctrl+C and
  Ctrl+Shift+C copy. Escape closes it. Ctrl+Shift+Left and Right stay
  what they are beside a second pane, the way to that pane: there a
  selection is extended by words with the pointer. A line that is longer
  than the pane is wide is cut at its edge: Shift with the wheel, or a
  wheel or a touchpad that scrolls sideways, moves the text sideways.
  Up to 8 MiB of a file are shown, a
  file that is not text says what it is and how large, and a file that
  changes on the disk is read again.
- **A file is changed in its pane, and saved.** Type into it, Enter
  keeps the indent of the line, Backspace and Delete take out a
  character or with Control a word. Ctrl+Z takes a step back and
  Ctrl+Shift+Z makes it again, Ctrl+X cuts, Ctrl+V pastes, Ctrl+S
  saves. The name of a file with changes that are not saved has
  another color. Nothing takes the place of such a file without asking:
  another file, the changes, closing the pane or the window. The lines
  at the bottom of the pane then name the keys, Ctrl+S saves and goes
  on, Ctrl+Shift+Backspace lets the changes go, Escape stays. A file
  that another program wrote to meanwhile is not written over: the
  lines say so, Ctrl+R takes what is on the disk, as a step that can be
  taken back, and Ctrl+Shift+S writes what is in the pane. A file that
  is not all text in UTF-8, or longer than is held, and a line over
  64 KiB are shown and not changed. What was typed since the last save
  is lost if codn ends without being asked to: no copy is kept.
- **A file in the colors of its language.** Comments, strings, numbers,
  keywords, functions, types and the names of a markup each have a
  color, in nineteen languages, told by the ending of the name: JSON,
  Python, TypeScript and TSX, Markdown, JavaScript, HTML, CSS, C++ and
  C, Rust, YAML, the shell, XML and SVG, TOML, QML, SQL, diffs and
  Makefiles. A file of Astro is shown as HTML and MDX as Markdown. The
  text is there at once and the colors a moment later, a quarter of a
  second for the first file of a language. A file over 1 MiB has one
  color, and so has what follows a line over 16 KiB. The colors are
  found by a second process of codn, `codn --syntax`, which runs while
  a file is shown: see "What codn runs on its own" in the README.
- **What changed in a file since the last commit, in the file.** A line
  that is new or changed has the surface it has in the changes. Where
  lines went, the number of the line behind that place is red. A file
  the last commit does not have says "new" beside its name. A click on
  the number of a marked line opens the changes at that file and line,
  and a click on the row of a file in the changes shows the file.

- **A program can say itself what it is doing.** codn takes the reports
  of the program status protocol (OSC 7501) and answers a program that
  asks whether it does. Claude Code speaks it from 2.1.295 on. Such a
  session shows what its program says, whatever it prints meanwhile:
  that it works, waits for you, is done or failed. The notification
  says which: that it asks for permission, asks a question, needs a
  login, is done or failed, each once until you looked at the session.
  What is done counts as seen once you looked. A program that reports nothing is told by its output
  and its silence, as before.
- **What a session is doing has a mark of its own shape.** In place of
  the dot: a gray ring at rest, a yellow arc that turns at work, a blue
  question mark where the program waits for you, a green check where it
  is done, a red triangle where it failed or is not running, and a blue
  dot where it stopped and did not say why. The shapes tell them apart
  without the colors.

- **The sidebar folds to a strip.** The first of the icons at the top
  right of the window, or Ctrl+Shift+B, folds it and brings it back,
  and codn starts as it was left. The strip keeps of every session the
  mark of what it is doing, so that a session that waits for you is
  still seen, and a click opens it. The list that starts a session or
  adds a project, the field that renames one and a message are shown in
  the whole sidebar, which lies over the pane for as long as they are
  there: no terminal gets another size for a list.

### Changed

- **What is shown beside the sessions has its icons in one place**, at
  the top right of the window beside its buttons: the sidebar, the
  changes, a second pane, the column with the files. They stand there
  whatever is shown, and one that has nothing to do right now steps
  back. The changes and the second pane had theirs in the header of a
  pane and moved with it. The name of the project in the column with
  the files stands in a row of its own, below them, and so do the
  title of a pane and what closes it where the window is too narrow
  for both beside the icons.
- **Which program a session runs is said in the header of its pane**,
  by its mark before the title, and in the list a session is started
  from. It used to stand before every name in the sidebar, where the
  rows now have the mark of what a session is doing and more room for
  its name.
- The header of a session no longer names its project beside the
  title: the sidebar shows it. It does while the sidebar is folded to
  its strip, which has no names.
- **Of two panes, the one that has the keyboard is plain to see.** Its
  header has a lighter surface, and what the other pane shows steps
  back a little. It used to be told by the color of the title alone.
- **The changes of a second project take the place of the first's.**
  Opened while nothing else was shown, they used to stand side by side.
- Two quick clicks at two places of a terminal are two clicks. They used
  to select a word, as a double click does.
- In a project that is a folder inside a repository, the changes name
  a new file from the top of the repository, as they name every other.
- Of two panes in a window of odd width, the right one takes the pixel
  that was left over at the right edge.
- The colors of a file come sooner while it is scrolled or typed in:
  a line that was colored a moment ago is not worked out a second time.

## 0.1.1

The bar along the bottom: what the program of a session reports about
itself, for Claude Code and for Codex.

### Added

- **A bar along the bottom edge of the window.** It shows what the program
  of the session in front reports about itself: at the left the model and
  what it runs with, such as the effort, and at the right one ring for the
  context and one for each limit, filled as far as it is used, with the
  time of day at which a limit is whole again. Where a program reports
  neither model nor options, the bar shows what you chose before the
  start. In a narrow window the rings give up what stands beside them
  before one of them goes.
- **A file to report in, for any program.** codn names a file to every
  program it starts, in the variable `CODN_STATUS`, and shows the few keys
  written there as TOML. A line of shell does as well as an agent.
- **Claude Code fills the bar** through the plugin in `mods/codn-status`,
  which you load yourself: the model, the effort and whether it thinks,
  the context and the limits.
- **Codex fills the bar** through `mods/codn-codex/report.py`, which you
  name as its `notify` in the arguments of the agent: the model and its
  effort, the context and both limits, each time a turn has ended.
- `{status}` in the arguments of an agent stands for the file codn names
  to that start, for a program whose reporter does not run with the
  environment of its session.

### Changed

- `+ Add project` is at the left of the bar along the bottom, and a second
  click on it closes the list it opened. The list of projects reaches down
  to the bar, and a terminal has 28 pixels less height.
- An agent in the configuration that is named like a built-in one, claude,
  codex or shell, keeps what it does not say: `name = "codex"` and one
  more line change that line and nothing else. It used to have only what
  the file said, so a file had to repeat the models and efforts of an
  agent to keep them, and then missed those built in later. A file that
  names such an agent with less than is built in now gets the rest.
- A new mark, in the header of the sidebar and as the application icon: a C
  of two bands that meet in one point. Install `assets/icon.svg` again to
  see it in the application menu.

### Security

- Every start of a program gets a file of its own to report in, so that
  nothing an earlier one left or still writes is taken for the next one's.
- Of such a file only numbers, names and times of day are shown, each
  within bounds. At most 16 KiB of it are read, and none that nests
  deeper than 64 levels.

## 0.1.0

The first version: a workspace for terminal coding agents on Linux, on
Wayland and on X11.

### Added

- **Projects and sessions** in a sidebar. A session is the agent's own
  command line program in a terminal: Claude Code, Codex or a plain shell
  are built in, any other program takes a few lines of configuration.
- **Starting a session from one list**: the agents, the one used last in the
  project first, and what can be set before the start, such as the model,
  the effort and the permission mode. What was set is remembered per project
  and agent.
- **Earlier conversations** of a project, found where the agents keep them
  and continued with a click, whether they were started in codn or not.
- **Restoring after a restart**: projects, sessions and their names come
  back, and agents that support it continue their conversation.
- **Status per session**: working, waiting, idle, not running. It is derived
  from terminal activity and needs no setup. A desktop notification tells
  when a session that is not on screen stopped.
- **Split view** with two sessions side by side.
- **Git state** per project and a **diff view** of everything that changed,
  including new files.
- **Adding a project** from a list of the git repositories below the home
  directory, most recent first, with a filter and a folder browser.
- **Session names** taken from the terminal title, numbered where they would
  read the same, and renamed by hand.
- **The terminal**: text styles, 256 and true colors, box-drawing and block
  characters drawn as geometry, color emoji, scrollback, selection, copy and
  paste, font size by shortcut. Shortcuts follow key positions on layouts
  without Latin letters.
- **Drawn only when something changed**: an idle window uses no CPU. What
  is typed shows without delay, also while a session prints, and a
  program whose output is expensive to take in does not hold up the
  window.
- **A window drawn by codn itself**, with its own title strip and buttons,
  and a single instance: starting codn again adds the named folders to the
  window that runs.
- **Configuration** in `~/.config/codn/config.toml`: fonts, scrollback,
  where to look for repositories, the agents with their options and the
  description of where they keep their conversations.
- **Icons** from [Codicons](https://github.com/microsoft/vscode-codicons),
  drawn for the size they have.
- **A mark and an application icon**, with a desktop entry to install, so
  that codn is found in the application menu and shown in the task bar.

### Security

- Git is asked for the state of a project with everything switched off that
  a repository could use to run a program: hooks, file system monitors,
  filter and diff drivers of the repository whatever they are called, lazy
  fetches. A repository that points at another folder as its work tree is
  not read.
- What a program prints in a terminal cannot end codn, and with it the
  other sessions.
- A program in a terminal can write to the clipboard only in a session that
  is on screen, and can never read it.
- Sessions do not inherit the variables of an agent that codn itself was
  started from.
- State is written atomically and readable only by its owner. Input, state,
  configuration, history and clipboard transfers have size limits.

[Unreleased]: https://github.com/hoffmannroman/codn/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/hoffmannroman/codn/releases/tag/v0.3.0
