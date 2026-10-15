# codn patch of smithay-clipboard 0.7.3

Source: crates.io smithay-clipboard 0.7.3, MIT (see LICENSE),
https://github.com/Smithay/smithay-clipboard.
Package checksum: `71704c03f739f7745053bde45fa203a46c58d25bc5c4efba1d9a60e9dba81226`.
Upstream example/dev targets are omitted; runtime API and dependencies are retained.

- Each load has its own reply channel; a delayed owner cannot answer a later load.
- Selection transfers expire after two seconds and retain at most 8 MiB.
- Read dispatch yields every 64 KiB so continuously writing owners cannot prevent
  timeout or worker shutdown. The caller also has a three-second reply deadline.

Also different from upstream, found when the diff was read again on
2026-10-06: text that is not UTF-8 is converted with replacement characters
on every path, and an error from registering the pipe or its timer is
passed on instead of dropped.

codn tells its user why a paste brought nothing, and knows a read that was
late by "did not answer" and one that was too large by "exceeds 8 MiB" in
what the error says: the texts in `src/state.rs` and `src/lib.rs` stay.

The public API is unchanged. This patch is selected through Cargo's crates.io
patch section; updates must retain these guarantees or replace them upstream.
