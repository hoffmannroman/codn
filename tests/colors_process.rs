//! codn started as its process for the colors, the real binary, and
//! spoken to as the app speaks to it. What is said is written out here
//! by hand, not taken from the app's code: it is read a second time.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

fn start() -> (Child, ChildStdin, ChildStdout) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codn"))
        .arg("--syntax")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let (to, from) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    (child, to, from)
}

fn ask(to: &mut ChildStdin, from: &mut ChildStdout, body: &[u8]) -> Vec<u8> {
    to.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
    to.write_all(body).unwrap();
    let mut head = [0; 4];
    from.read_exact(&mut head).unwrap();
    let mut answer = vec![0; u32::from_le_bytes(head) as usize];
    from.read_exact(&mut answer).unwrap();
    answer
}

fn text(id: u64, grammar: &str, text: &[u8]) -> Vec<u8> {
    [&[1][..], &id.to_le_bytes(), &[grammar.len() as u8], grammar.as_bytes(), text].concat()
}

fn lines(id: u64, from: u32, count: u32) -> Vec<u8> {
    [&[2][..], &id.to_le_bytes(), &from.to_le_bytes(), &count.to_le_bytes()].concat()
}

fn ended(child: &mut Child) -> std::process::ExitStatus {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < until, "it does not end");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn it_cuts_lines_into_pieces_and_ends_when_nobody_asks_any_more() {
    let (mut child, mut to, mut from) = start();
    assert_eq!(
        ask(&mut to, &mut from, &text(7, "Rust", b"// a\nfn b() {}\n")),
        [&[1][..], &7u64.to_le_bytes()].concat()
    );
    let answer = ask(&mut to, &mut from, &lines(7, 0, 9));
    // Pieces, of the text 7, from line 0, nothing unknown, three lines:
    // a comment of five bytes, then `fn` as a keyword up to byte 2.
    let head =
        [&[2][..], &7u64.to_le_bytes(), &0u32.to_le_bytes(), &u32::MAX.to_le_bytes(), &3u32.to_le_bytes()];
    assert_eq!(answer[..21], head.concat());
    assert_eq!(answer[21..30], [&1u32.to_le_bytes()[..], &5u32.to_le_bytes(), &[1]].concat());
    assert_eq!(answer[34..39], [&2u32.to_le_bytes()[..], &[4]].concat());
    assert_eq!(answer[answer.len() - 4..], 0u32.to_le_bytes(), "the empty last line has no pieces");
    // What is no question is refused, and the process goes on.
    assert_eq!(ask(&mut to, &mut from, b"nonsense"), [5]);
    assert_eq!(ask(&mut to, &mut from, &lines(8, 0, 1)), [5]);
    assert_eq!(ask(&mut to, &mut from, &lines(7, 2, 1))[0], 2);
    drop(to);
    assert!(ended(&mut child).success());
}

#[test]
fn it_holds_nothing_of_what_the_app_had_open_and_has_a_limit_on_its_memory() {
    // A file the process would inherit: opened so that it stays open
    // in a program that is started, as the PTY of a session is.
    let file = std::fs::File::open("/proc/self/status").unwrap();
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) };
    let (mut child, mut to, mut from) = start();
    // It has answered: what it does at its start is done.
    assert_eq!(ask(&mut to, &mut from, &text(1, "Rust", b"")), [&[1][..], &1u64.to_le_bytes()].concat());
    let mut open: Vec<String> = std::fs::read_dir(format!("/proc/{}/fd", child.id()))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    open.sort();
    assert_eq!(open, ["0", "1", "2"]);
    let limits = std::fs::read_to_string(format!("/proc/{}/limits", child.id())).unwrap();
    let limit = |name: &str| limits.lines().find(|line| line.starts_with(name)).map(str::to_owned).unwrap();
    assert!(limit("Max address space").contains(" 1073741824 "), "{}", limit("Max address space"));
    // Both of them, the soft one and the hard one: a core of nothing is
    // what many a shell hands on as the soft one, set or not.
    let core: Vec<String> =
        limit("Max core file size").split_whitespace().skip(4).take(2).map(str::to_owned).collect();
    assert_eq!(core, ["0", "0"], "{}", limit("Max core file size"));
    drop(file);
    drop(to);
    ended(&mut child);
}

#[test]
fn a_question_longer_than_any_ends_it_and_so_does_one_that_breaks_off() {
    let (mut child, mut to, _from) = start();
    to.write_all(&u32::MAX.to_le_bytes()).unwrap();
    to.flush().unwrap();
    assert!(!ended(&mut child).success());

    let (mut child, mut to, _from) = start();
    to.write_all(&100u32.to_le_bytes()).unwrap();
    to.write_all(b"half").unwrap();
    drop(to);
    assert!(!ended(&mut child).success());
}

#[test]
fn it_says_what_a_text_of_markdown_is_made_of_and_holds_no_text_for_it() {
    let (mut child, mut to, mut from) = start();
    // A question for a structure: its kind, a number, the text.
    let outline = |id: u64, text: &[u8]| [&[4][..], &id.to_le_bytes(), text].concat();
    let answer = ask(&mut to, &mut from, &outline(5, b"# a\n"));
    // A structure, for the question 5, of three entries: a heading of
    // the first level over the bytes 0 to 4, its text, which is the
    // byte 2, and its end.
    let head = [&[6][..], &5u64.to_le_bytes(), &3u32.to_le_bytes()].concat();
    let heading = [&[2][..], &0u32.to_le_bytes(), &4u32.to_le_bytes(), &[1]].concat();
    let word =
        [&[19][..], &2u32.to_le_bytes(), &3u32.to_le_bytes(), &[0], &2u32.to_le_bytes(), &3u32.to_le_bytes()]
            .concat();
    let end = [&[0][..], &0u32.to_le_bytes(), &4u32.to_le_bytes()].concat();
    assert_eq!(answer, [head, heading, word, end].concat());
    // What is no text has none, and the process goes on.
    assert_eq!(ask(&mut to, &mut from, &outline(6, b"\xff")), [&[7][..], &6u64.to_le_bytes(), &[1]].concat());
    // It held no text for that: lines are refused until one is given.
    assert_eq!(ask(&mut to, &mut from, &lines(5, 0, 1)), [5]);
    assert_eq!(ask(&mut to, &mut from, &text(7, "Markdown", b"# a\n"))[0], 1);
    assert_eq!(ask(&mut to, &mut from, &lines(7, 0, 1))[0], 2);
    drop(to);
    assert!(ended(&mut child).success());
}
