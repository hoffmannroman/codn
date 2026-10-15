# Local VTE parser patch

Source: crates.io `vte` 0.15.0, https://github.com/alacritty/vte.
Package checksum: `a5924018406ce0063cd67f8e008104968b74b563ee1b85dde3ed1f7cb87d3dbd`. Dual Apache-2.0/MIT licenses are retained
in `LICENSE-APACHE` and `LICENSE-MIT`. Upstream Rust requirement is retained;
the patch uses no newer language/API features. Source and crate manifest
are retained; the unused example manifest entry is removed.

The std parser used by alacritty stores OSC payloads in a Vec without a
limit. Bound raw payloads to 8 MiB, including unterminated commands. On
overflow clear/discard the entire command, continue recognizing BEL or
ESC-backslash ST, and recover for following commands and ordinary text.
Do not dispatch a truncated title or clipboard command. Non-std behavior
is unchanged.

`Handler::max_repeat` says how many characters one repetition (REP,
`CSI n b`) may print, and the parser asks it. The default is upstream's
65,535; the terminal answers with the size of its screen, see the patches
of `alacritty_terminal`.

An OSC that no handler takes was put into words for the log before the
log was asked whether it takes them: three bytes and more for every byte
of a payload, 24 MiB for the largest, built while the terminal is locked.
Nothing of it is built unless a logger takes `debug` (`unhandled` in
`src/ansi.rs`). codn sets none.

`Handler::program_status` is handed what a program says of its own state
(OSC 7501): what stood behind the number, in the pieces a `;` cut it
into, and how the command was ended. `Handler::prompt_start` is told
where a shell begins its prompt (OSC 133 ; A). Both do nothing unless a
handler takes them. Upstream passes both to the log.

Not behavior: upstream's `rustfmt.toml` is not kept and the sources are
formatted with codn's settings, which is most of a diff against upstream,
`src/ansi.rs` included. `doc/` and `examples/` are omitted.

Validation: upstream parser/ANSI unit tests plus oversized, unterminated
OSC recovery for both terminators and CAN/SUB cancellation, a test that
counts what the end of an OSC nobody handles asks of the allocator, and
one each for the state of a program and the start of a prompt.
