//! What git prints for a status and for a diff, read into what is shown.

use super::{Info, Kind, Line, Marks};

/// Whether the name of a file stays in its repository: names joined by
/// one slash each, no path from the root, and no step up or on the spot
/// in it. Nothing between two slashes is no name: a path reads over it,
/// and a name that is cut behind a folder, as the changes of a project
/// inside its repository are, would begin at the root.
pub fn inside(name: &str) -> bool {
    name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// Takes out the changes of files whose names lead out of the
/// repository, and says how many those were. git names no such file by
/// itself, but an index can be written that does, and its lines would
/// be those of a file anywhere on the disk. A note is no part of a file.
pub(super) fn within(lines: &mut Vec<Line>) -> usize {
    let (mut kept, mut outside) = (true, 0);
    lines.retain(|line| {
        if line.names_file() {
            kept = inside(&line.text);
            outside += usize::from(!kept);
        }
        kept || line.kind == Kind::Note
    });
    outside
}

const MAX_LINES: usize = 50_000;
/// Longest line kept. No pane is that wide, and a line of megabytes, as
/// minified files have them, would be measured on every frame.
const MAX_COLUMNS: usize = 1000;

pub(super) fn parse_status(out: &str) -> Option<Info> {
    let (mut branch, mut oid, mut changed) = (None, "", 0);
    for line in out.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            branch = Some(head);
        } else if let Some(id) = line.strip_prefix("# branch.oid ") {
            oid = id;
        } else if !line.starts_with('#') && !line.is_empty() {
            changed += 1;
        }
    }
    let branch = match branch? {
        "(detached)" => oid.chars().take(7).collect(),
        name => name.to_owned(),
    };
    Some(Info { branch, changed })
}

/// A file name as git prints it in a diff header: without the trailing tab
/// it adds to names with spaces, and with the quoting undone that it puts
/// around unusual names.
fn file_name(name: &str) -> String {
    let name = name.trim_end_matches('\t');
    let Some(quoted) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
        return name.to_owned();
    };
    // Git quotes names with C-style escapes even with core.quotePath=false.
    // Decode once: chained replacements confuse a literal backslash+t with
    // a tab, and miss newline/CR/octal escapes entirely.
    let mut bytes = Vec::with_capacity(quoted.len());
    let mut chars = quoted.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('a') => bytes.push(7),
            Some('b') => bytes.push(8),
            Some('t') => bytes.push(9),
            Some('n') => bytes.push(10),
            Some('v') => bytes.push(11),
            Some('f') => bytes.push(12),
            Some('r') => bytes.push(13),
            Some('"') => bytes.push(b'"'),
            Some('\\') => bytes.push(b'\\'),
            Some(c @ '0'..='7') => {
                let mut value = c.to_digit(8).unwrap();
                for _ in 0..2 {
                    if let Some(c @ '0'..='7') = chars.peek().copied() {
                        chars.next();
                        value = value * 8 + c.to_digit(8).unwrap();
                    } else {
                        break;
                    }
                }
                bytes.push(value as u8);
            }
            Some(c) => {
                bytes.push(b'\\');
                let mut buffer = [0; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A line as it is shown: tabs as four spaces, and cut at `MAX_COLUMNS`.
fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_COLUMNS));
    let mut room = MAX_COLUMNS;
    for c in text.chars() {
        let width = if c == '\t' { 4.min(room) } else { 1 };
        if room == 0 {
            break;
        }
        if c == '\t' {
            out.push_str(&"    "[..width])
        } else {
            out.push(c)
        }
        room -= width;
    }
    out
}

/// Adds the lines of git's diff output to `lines`. Returns whether there is
/// room for more, and says so in the list when there is not.
pub(super) fn parse_diff(lines: &mut Vec<Line>, out: &str) -> bool {
    let (mut file, mut in_hunk, mut old, mut new) = (None, false, 0, 0);
    for raw in out.lines() {
        if lines.len() >= MAX_LINES {
            lines.push(Line::note("diff too long, cut off here"));
            return false;
        }
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(names) = raw.strip_prefix("diff --git ") {
            // A first guess. The lines that follow name the file exactly,
            // where there are any: a change of mode alone has none.
            let quoted = names.rfind(" \"b/").filter(|_| names.ends_with('"'));
            // Without quotes the line is the name twice, behind `a/` and
            // behind ` b/`, and the name may hold a ` b/` itself: it is
            // the half of what is left without those five characters.
            let twice = names.len().checked_sub(5).filter(|rest| rest % 2 == 0).map(|rest| rest / 2);
            let halves = twice.and_then(|length| {
                let (first, second) = (names.get(2..2 + length)?, names.get(length + 5..)?);
                let parted = names.starts_with("a/") && names.get(2 + length..length + 5) == Some(" b/");
                (parted && first == second).then_some(first)
            });
            let path = match (quoted, halves) {
                (Some(at), _) => file_name(&names[at + 1..]).split_off(2),
                (None, Some(name)) => file_name(name),
                (None, None) => file_name(names.rsplit_once(" b/").map_or(names, |(_, path)| path)),
            };
            file = Some(lines.len());
            in_hunk = false;
            lines.push(Line { kind: Kind::File, old: 0, new: 0, text: path });
        } else if let Some(range) = raw.strip_prefix("@@ -") {
            // "@@ -12,7 +14,9 @@ context"
            let start = |s: &str| s.split([',', ' ']).next().and_then(|n| n.parse().ok()).unwrap_or(0);
            old = start(range);
            new = range.split_once(" +").map_or(0, |(_, rest)| start(rest));
            in_hunk = true;
            // Between hunks this row marks the gap, at the top of a file it
            // only earns its place when it names a context.
            let context = raw.splitn(3, "@@").nth(2).unwrap_or("").trim();
            if !context.is_empty() || lines.last().is_some_and(|l| !l.names_file()) {
                lines.push(Line { kind: Kind::Hunk, old: 0, new: 0, text: context.to_owned() });
            }
        } else if in_hunk {
            let mut chars = raw.chars();
            let Some(sign) = chars.next() else { continue };
            let (kind, a, b) = match sign {
                '+' => (Kind::Add, 0, new),
                '-' => (Kind::Del, old, 0),
                ' ' => (Kind::Context, old, new),
                _ => continue,
            };
            old += (kind != Kind::Add) as u32;
            new += (kind != Kind::Del) as u32;
            if let Some(file) = file {
                lines[file].new += (kind == Kind::Add) as u32;
                lines[file].old += (kind == Kind::Del) as u32;
            }
            lines.push(Line { kind, old: a, new: b, text: shown(chars.as_str()) });
        } else if let (Some(file), true) = (file, raw.starts_with("deleted file mode ")) {
            lines[file].kind = Kind::Gone;
        } else if let (Some(file), Some(name)) = (file, header_name(raw)) {
            lines[file].text = name;
        } else if raw.starts_with("Binary files ") {
            lines.push(Line::note("binary file"));
        }
    }
    lines.len() < MAX_LINES
}

/// Most hunks of one file that are taken in.
const MAX_HUNKS: usize = 100_000;

/// Reads what changed in one file from a diff of it that was made without
/// lines of context. Only the heads of its hunks are read: they say
/// which lines of the file as it is are new or changed, and where lines
/// went, which the lines of the diff themselves no longer tell. Nothing
/// for a diff of more hunks than are taken in.
pub(super) fn parse_marks(out: &str) -> Option<Marks> {
    let mut marks = Marks::default();
    // "12", "12,3": where a side begins, and how many lines of it, one
    // where that is not said.
    let side = |text: &str| {
        let (start, count) = text.split_once(',').unwrap_or((text, "1"));
        Some((start.parse::<u32>().ok()?, count.parse::<u32>().ok()?))
    };
    for raw in out.lines() {
        // A file that the last commit does not have is new as a whole.
        if raw.starts_with("new file mode ") && marks.changed.is_empty() && marks.gone.is_empty() {
            marks.new = true;
        }
        // "@@ -12,7 +14,9 @@ context"
        let Some(range) = raw.strip_prefix("@@ -") else { continue };
        let Some((_, new)) = range.split_once(" +") else { continue };
        let Some((start, count)) = new.split([' ', '@']).next().and_then(side) else { continue };
        // More of them than are taken in: the first ones alone would
        // say that the rest of the file did not change.
        if marks.changed.len() + marks.gone.len() >= MAX_HUNKS {
            return None;
        }
        if count == 0 {
            // "+12,0": lines went behind line 12, which is before the
            // line that is counted as 12 from nothing.
            marks.gone.push(start);
        } else {
            let first = start.saturating_sub(1);
            marks.changed.push((first, first.saturating_add(count)));
        }
    }
    Some(marks)
}

/// The file a header line between "diff --git" and the first hunk names.
/// Later lines win: the new name of a rename, the new side of a change.
fn header_name(line: &str) -> Option<String> {
    if let Some(name) = line.strip_prefix("rename to ") {
        return Some(file_name(name));
    }
    let name = file_name(line.strip_prefix("+++ ").or_else(|| line.strip_prefix("--- "))?);
    // The names carry an "a/" or "b/" in front, and a side that does not
    // exist is /dev/null.
    name.strip_prefix("a/").or_else(|| name.strip_prefix("b/")).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_counts_changes_and_names_the_branch() {
        let out = "# branch.oid 1234567890abcdef\n# branch.head main\n1 .M N... 100644 100644 100644 a b src/x.rs\n? new.txt\n";
        assert_eq!(parse_status(out), Some(Info { branch: "main".into(), changed: 2 }));
        let detached = "# branch.oid 1234567890abcdef\n# branch.head (detached)\n";
        assert_eq!(parse_status(detached), Some(Info { branch: "1234567".into(), changed: 0 }));
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn diff_tracks_line_numbers_and_file_totals() {
        let out = "diff --git a/src/x.rs b/src/x.rs\nindex 1..2 100644\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -10,3 +10,4 @@ fn x() {\n keep\n-gone\n+new\n+\tnewer\n keep\n";
        let mut lines = Vec::new();
        assert!(parse_diff(&mut lines, out));
        let kinds: Vec<Kind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [Kind::File, Kind::Hunk, Kind::Context, Kind::Del, Kind::Add, Kind::Add, Kind::Context]
        );
        assert_eq!((lines[0].text.as_str(), lines[0].old, lines[0].new), ("src/x.rs", 1, 2));
        assert_eq!((lines[3].old, lines[3].new), (11, 0));
        assert_eq!((lines[5].old, lines[5].new, lines[5].text.as_str()), (0, 12, "    newer"));
        assert_eq!((lines[6].old, lines[6].new), (12, 13));
    }

    #[test]
    fn diff_stops_at_its_limits() {
        let long = format!("diff --git a/x b/x\n@@ -1 +1 @@\n+{}\n", "y".repeat(MAX_COLUMNS + 50));
        let mut lines = Vec::new();
        assert!(parse_diff(&mut lines, &long));
        assert_eq!(lines.last().unwrap().text.chars().count(), MAX_COLUMNS);

        // Tabs count as the spaces they become.
        let tabs = format!("diff --git a/x b/x\n@@ -1 +1 @@\n+ab{}\n", "\t".repeat(MAX_COLUMNS));
        let mut lines = Vec::new();
        parse_diff(&mut lines, &tabs);
        let text = &lines.last().unwrap().text;
        assert_eq!(text.chars().count(), MAX_COLUMNS);
        assert!(text.starts_with("ab    ") && text.ends_with("  "));

        // A list that is exactly full has no room for another file.
        let full = format!("diff --git a/x b/x\n@@ -1 +1 @@\n{}", "+y\n".repeat(MAX_LINES - 1));
        let mut lines = Vec::new();
        assert!(!parse_diff(&mut lines, &full));
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines.last().unwrap().kind, Kind::Add);

        let many = format!("diff --git a/x b/x\n@@ -1 +1 @@\n{}", "+y\n".repeat(MAX_LINES + 10));
        let mut lines = Vec::new();
        assert!(!parse_diff(&mut lines, &many));
        assert_eq!(lines.len(), MAX_LINES + 1);
        assert_eq!(lines.last().unwrap().kind, Kind::Note);
    }

    #[test]
    fn a_name_that_stays_inside_is_names_joined_by_one_slash_each() {
        for name in ["a", "a/b.txt", "deep/in/it", ".hidden", "a..b", "..."] {
            assert!(inside(name), "{name}");
        }
        // Nothing, from the root, up or on the spot, and what is no
        // name between two slashes or behind the last: cut behind a
        // folder, `app//etc` is a path from the root.
        for name in
            ["", "/", "/etc/hosts", "..", "../a", "a/..", "a/../b", ".", "./a", "a/./b", "a//b", "a/", "//"]
        {
            assert!(!inside(name), "{name}");
        }
    }

    #[test]
    fn the_changes_of_files_outside_the_repository_are_taken_out() {
        let file = |name: &str| {
            format!(
                "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-x\n+y of {name}\n"
            )
        };
        let out =
            [file("in.txt"), file("../outside.txt"), file("deep/in.txt"), file("a/../../b.txt")].concat();
        let mut lines = vec![Line::note("Unstaged changes")];
        assert!(parse_diff(&mut lines, &out));
        // A name from the root, as an index can hold it.
        lines.push(Line { kind: Kind::File, old: 0, new: 1, text: "/etc/hosts".into() });
        lines.push(Line { kind: Kind::Add, old: 0, new: 1, text: "a line of it".into() });
        lines.push(Line::note("A note behind them stays"));
        assert_eq!(within(&mut lines), 3);
        let left: Vec<&str> = lines.iter().filter(|l| l.names_file()).map(|l| l.text.as_str()).collect();
        assert_eq!(left, ["in.txt", "deep/in.txt"]);
        assert!(
            !lines
                .iter()
                .any(|l| l.text.contains("outside") || l.text.contains("b.txt") || l.text == "a line of it")
        );
        let notes: Vec<&str> =
            lines.iter().filter(|l| l.kind == Kind::Note).map(|l| l.text.as_str()).collect();
        assert_eq!(notes, ["Unstaged changes", "A note behind them stays"]);
    }

    #[test]
    fn diff_names_files_exactly() {
        let out = "diff --git a/a b/c.txt b/a b/c.txt\n--- a/a b/c.txt\t\n+++ b/a b/c.txt\t\n@@ -1 +1 @@\n-x\n+y\n\
                   diff --git a/old.rs b/new.rs\nsimilarity index 90%\nrename from old.rs\nrename to new.rs\n\
                   diff --git a/gone b/gone\ndeleted file mode 100644\n--- a/gone\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n\
                   diff --git \"a/q\\\"uote\" \"b/q\\\"uote\"\n--- \"a/q\\\"uote\"\n+++ \"b/q\\\"uote\"\n@@ -1 +1 @@\n-x\n+\u{fc}ber\n";
        let mut lines = Vec::new();
        assert!(parse_diff(&mut lines, out));
        let files: Vec<&str> = lines.iter().filter(|l| l.names_file()).map(|l| l.text.as_str()).collect();
        assert_eq!(files, ["a b/c.txt", "new.rs", "gone", "q\"uote"]);
        assert_eq!(lines.last().unwrap().text, "\u{fc}ber");

        // A change of mode has no line after the first that names the file.
        let mode = "diff --git \"a/ta\\tb.sh\" \"b/ta\\tb.sh\"\nold mode 100644\nnew mode 100755\n\
                    diff --git a/plain.sh b/plain.sh\nold mode 100644\nnew mode 100755\n";
        let mut lines = Vec::new();
        assert!(parse_diff(&mut lines, mode));
        let files: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(files, ["ta\tb.sh", "plain.sh"]);
        // Nor has it one where the name holds what parts the two halves
        // of that first line: a space and the letter of the second.
        let spaced = "diff --git a/a b/c.txt b/a b/c.txt\nold mode 100644\nnew mode 100755\n\
                      diff --git a/ b/lead.txt b/ b/lead.txt\nold mode 100644\nnew mode 100755\n";
        let mut lines = Vec::new();
        assert!(parse_diff(&mut lines, spaced));
        let files: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(files, ["a b/c.txt", " b/lead.txt"]);
    }

    #[test]
    fn quoted_filenames_decode_once_and_include_control_and_octal_escapes() {
        assert_eq!(file_name(r#""a\\tab""#), r"a\tab");
        assert_eq!(file_name(r#""a\ttab""#), "a\ttab");
        assert_eq!(file_name(r#""a\nline\rreturn""#), "a\nline\rreturn");
        assert_eq!(file_name(r#""caf\303\251""#), "café");
        assert_eq!(header_name(r#"+++ "b/a\\tab""#).as_deref(), Some(r"a\tab"));
    }

    #[test]
    fn the_heads_of_the_hunks_say_which_lines_changed_and_where_lines_went() {
        let diff = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n\
            @@ -1 +0,0 @@\n-gone\n@@ -4 +3,2 @@ fn main() {\n-old\n+new\n+more\n\
            @@ -9,2 +9,0 @@\n-a\n-b\n@@ -20,0 +19 @@\n+added\n";
        let marks = parse_marks(diff).unwrap();
        // Lines 3 and 4 and line 19 of the file, counted from nothing.
        assert_eq!(marks.changed, [(2, 4), (18, 19)]);
        // Before the first line, and before the tenth.
        assert_eq!(marks.gone, [0, 9]);
        assert!(!marks.new);
        assert!(marks.is_changed(2) && marks.is_changed(3) && !marks.is_changed(4) && marks.is_changed(18));
        assert!(marks.is_gone_before(0) && marks.is_gone_before(9) && !marks.is_gone_before(1));

        // A file the last commit does not have, and one that was emptied.
        let added = parse_marks(
            "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+a\n+b\n",
        )
        .unwrap();
        assert!(added.new && added.changed == [(0, 2)]);
        assert_eq!(parse_marks("@@ -1,3 +0,0 @@\n-a\n-b\n-c\n").unwrap().gone, [0]);
        // What is no head of a hunk says nothing, whatever it looks like.
        let odd = parse_marks("@@ nothing\n@@ -1 +x @@\n@@ -1 +4294967296 @@\n+@@ -1 +2 @@\n");
        assert_eq!(odd, Some(Marks::default()));
        assert_eq!(parse_marks(""), Some(Marks::default()));
    }

    #[test]
    fn more_hunks_than_are_taken_in_are_no_marks_at_all() {
        let heads =
            |count: usize| (0..count).map(|n| format!("@@ -{0} +{0} @@\n", n * 2 + 1)).collect::<String>();
        assert_eq!(parse_marks(&heads(MAX_HUNKS)).map(|marks| marks.changed.len()), Some(MAX_HUNKS));
        // The first of them alone would say that the rest did not change.
        assert_eq!(parse_marks(&heads(MAX_HUNKS + 1)), None);
    }
}
