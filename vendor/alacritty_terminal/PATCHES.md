# Local terminal-library patches

Source: crates.io `alacritty_terminal` 0.26.0, upstream
https://github.com/alacritty/alacritty, Apache-2.0 (`LICENSE-APACHE`).
Package checksum: `bda177466b9524d59f1b12f0dd30b68696788e9992a7e959021c4a0ed96fcf59`. Source and crate manifest are retained;
the 47 MiB external reference-test corpus is omitted, with its manifest test
entry removed. Keep this document updated when rebasing onto upstream.

- Limit each PTY parse chunk to 64 KiB so the reader cannot parse a buffered
  1 MiB while holding the terminal lock.
- Bound retained PTY input across cloned senders and the write queue to 8 MiB.
  Reject the entire new message, preserve queued input and control messages,
  and report a notice through codn.
- Bound combining marks to 128 per cell.
- Trim unused trailing screen rows from non-rectangular selection copies,
  preserving interior blank rows and line selections' terminating newline.
  codn additionally limits selection extraction to 100,000 cells and 8 MiB.
- On Linux capture process identities with pidfds; clean up only the owned
  child and verified members of its PTY session. Give HUP two seconds, TERM
  two seconds, then KILL. An unreapable child is transferred to a detached
  reaper so the reader and emulator can drop. Already reaped children are
  never signalled by a stale numeric PID. Older kernels use `Child::kill`
  only while the owned child is unreaped.
- Propagate working-directory errors during spawn and unregister SIGCHLD
  handlers when spawning fails.
- A program started on a PTY inherits no file of the app: every
  descriptor from 3 on is marked to be closed when the program is
  started (`close_range` with `CLOSE_RANGE_CLOEXEC`, Linux). Upstream
  closes its own two and leaves what libraries of the app left open.
- Copying a row that begins with the second half of a wide letter does
  not step back below the first column (`line_to_string`): a build
  that checks arithmetic ended there, and another lost the row.
- A question for a color is answered with the color the program set,
  which the answer is given by the terminal itself: the formatter of
  `Event::ColorRequest` takes the color it is handed only where none is
  set. The listener is called while the terminal is locked, so whoever
  answered had to ask the terminal on another thread, and codn's reader
  answers itself now.
- Check `getpwuid_r` errors before reading its output buffer; return errors
  for invalid records or non-UTF-8 fields instead of panicking.
- When the caller has no native window id (zero), remove `WINDOWID` and
  `ALACRITTY_WINDOW_ID` instead of advertising a nonexistent Alacritty window.
- Take the oldest entry from the keyboard mode stack when it is full. Upstream
  removes it from the title stack instead, which panics when no title was
  pushed and otherwise leaves the keyboard mode stack unbounded.
- Move the view into the history only for lines that went into the history,
  and never past its end. Upstream moves it for every scroll, also below
  fixed lines at the top, until rows outside the storage are indexed.
- Keep at most 4096 bytes of a title, which the title stack holds up to 4096
  times.
- After SIGCHLD, look for the members of the session only when the session's
  own child ended, not for every child of the process.
- One repetition (REP, `CSI n b`) prints a screen of characters at most.
  Upstream prints up to 65,535 per request, and a chunk of output holds
  8,000 requests: 2.8 seconds under the terminal lock, measured.
- Hyperlinks (OSC 8) are not kept with the cells. Each cell printed under
  one held on to its address, of up to 8 MiB, for as long as it was in the
  history: 400 addresses of 1 MiB were 400 MiB. codn shows no hyperlinks;
  the text between the two commands is shown as before.
- One lock of the reader parses 64 KiB at most. A short read followed by a
  full one was two chunks under one lock.
- `FairMutex::try_lock_for` locks unless that takes longer than a limit, so
  that a frame need not wait for a chunk that is expensive to parse.
- What a program says of its own state (OSC 7501) is passed on as it came
  as `Event::ProgramStatus`, if its sequence has 4096 bytes at the most
  with what begins and ends it, and if it came in fewer pieces than the
  parser hands over: one in as many may be cut off behind the last, and the start of a
  shell's prompt (OSC 133 ; A) as `Event::PromptStart`. The question
  whether the terminal takes such reports, `OSC 7501 ; ?`, is answered
  with the question itself, ended as it was asked, and nothing else of
  such a command is ever written back. A terminal put back as it was
  built (RIS) says so with `ProgramStatus(None)`. Upstream knows neither
  command. The reports are read in codn (`src/core/program.rs`).

Not behavior: the sources are formatted with codn's rustfmt settings, which
is most of a diff against upstream. `READ_BUFFER_SIZE` is 64 KiB (the parse
chunk above), and `Event` has the two variants `InputRejected` and
`SelectionRejected` for the bounds above, and `ProgramStatus` and
`PromptStart` for the state of a program.

Validation: crate unit tests for input budget, combining marks, selection
copy, working-directory errors, identity-safe process cleanup, both stacks,
the view below fixed lines, the repetition, the hyperlink, the lock with
a limit and the state of a program; codn's full unit tests and isolated GUI stress checks. The crate's
tests need the patched parser:
`cargo test --config 'patch.crates-io.vte.path="../vte"'` in this folder,
with an absolute path, and remove the `Cargo.lock` it leaves.
