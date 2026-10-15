# Where the font comes from

`CalSansVF.ttf` is Cal Sans, the typeface the word codn in the logo is
set in. It is not codn's: it is by Mark Davis of Wordmark, commissioned
by Cal.com, and is used under the SIL Open Font License 1.1, which lies
beside it as `OFL.txt`.

| | |
| --- | --- |
| Source | <https://github.com/calcom/sans> |
| Commit | `706884a0420ad801f3c21ebe129059e97ae7180d` |
| File there | `fonts/calsans-var-full/CalSansVF.ttf`, the complete variable font |
| Version | 2.011 |
| SHA-256 | `9e1ae3cbe3b691b4e7611b7f66d10e0fdd930275b56866750d6207bd917f215a` |

The file is as it came. Nothing of it is built into codn: the app draws
its text in the fonts of the system. `scripts/logo.py` reads it to set
the word beside the mark, and what it writes holds the outlines of four
letters and no font.

To take a newer state: put the file here, change the commit, the version
and the sum on this page, and run the script. It stops where the font no
longer gives the measures the word is set by.
