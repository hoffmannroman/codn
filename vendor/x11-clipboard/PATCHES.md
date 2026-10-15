# codn patch of x11-clipboard 0.9.3

Source: crates.io x11-clipboard 0.9.3, MIT (see LICENSE),
https://github.com/quininer/x11-clipboard.
Package checksum: `662d74b3d77e396b8e5beb00b9cad6a9eccf40b2ef68cc858784b14c41d535a3`.
Source/manifest/license are retained; unused example targets are omitted.

Direct properties and incremental selections are limited to 8 MiB before
reserve, property fetch, or append. The incremental property byte count is
converted to X11's four-byte length units. A new non-exhaustive Error::TooLarge
reports an oversized selection. Existing public method signatures are unchanged.

A read waits for the connection to the server, 50 ms at the most, where
upstream slept 50 ms whenever no event was there yet. An owner that hands a
text over in pieces (INCR) answers each within a millisecond, and with a
sleep after every piece a read got 4000 bytes in 50 ms from an owner with
pieces of that size: under the deadline of three seconds that the caller
sets for the whole read, a text of more than about 240 KB came as nothing.

codn tells its user why a paste brought nothing, and knows `Error::Timeout`
and `Error::TooLarge` by what they say: a test here holds the two texts.

Upstream's `tests/` folder and its two test targets are omitted with the
examples. The bound and the wait have unit tests here; the protocol path has
none. It was run against a live selection owner, `xsel` on a virtual X
server, with texts of 1 KB to 8 MB: each arrived whole, 8 MB within 0.1 s.
