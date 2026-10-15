//! codn started as its process for a picture, the real binary, handed a
//! file and read as the app reads it. What it is told and what it answers
//! is written out here by hand, not taken from the app's code: it is read
//! a second time.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn sum(bytes: &[u8]) -> u32 {
    let mut sum = u32::MAX;
    for byte in bytes {
        sum ^= u32::from(*byte);
        for _ in 0..8 {
            sum = if sum & 1 == 1 { 0xedb8_8320 ^ (sum >> 1) } else { sum >> 1 };
        }
    }
    !sum
}

fn chunk(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let named = sum(&[&name[..], body].concat());
    [&(body.len() as u32).to_be_bytes()[..], name, body, &named.to_be_bytes()].concat()
}

/// A PNG of color with alpha, eight bits, of these rows.
fn png(w: u32, h: u32, rows: &[u8]) -> Vec<u8> {
    let raw: Vec<u8> = rows.chunks(w as usize * 4).flat_map(|row| [&[0][..], row].concat()).collect();
    let packed = yazi::compress(&raw, yazi::Format::Zlib, yazi::CompressionLevel::Default).unwrap();
    let head = [&w.to_be_bytes()[..], &h.to_be_bytes(), &[8, 6, 0, 0, 0]].concat();
    [&b"\x89PNG\r\n\x1a\n"[..], &chunk(b"IHDR", &head), &chunk(b"IDAT", &packed), &chunk(b"IEND", &[])]
        .concat()
}

fn written(name: &str, bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("codn-picture-process-{name}-{}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn start(told: &[&str], input: Stdio) -> Child {
    Command::new(env!("CARGO_BIN_EXE_codn"))
        .arg("--picture")
        .args(told)
        .stdin(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

const WHOLE: [&str; 9] = ["png", "100", "100", "4096", "8388608", "0", "0", "0", "0"];

/// Everything it writes, and how it ended.
fn answered(told: &[&str], file: &PathBuf) -> (Vec<u8>, bool) {
    let mut child = start(told, File::open(file).unwrap().into());
    let mut all = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut all).unwrap();
    (all, ended(&mut child).success())
}

fn ended(child: &mut Child) -> std::process::ExitStatus {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < until, "it does not end");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The length of a head.
const HEAD: usize = 40;

/// A head: the mark, the version, how it went, what is noted, the size of
/// the picture, the part of it that was made and the size of what follows.
fn head(how: u8, notes: u16, picture: (u32, u32), part: [u32; 4], size: (u32, u32)) -> Vec<u8> {
    let fields = [picture.0, picture.1, part[0], part[1], part[2], part[3], size.0, size.1]
        .map(u32::to_le_bytes)
        .concat();
    [&b"cdnp"[..], &[2, how], &notes.to_le_bytes(), &fields].concat()
}

#[test]
fn it_answers_a_head_and_the_pixels_and_ends() {
    // Red that is all there, green at half, and two of nothing.
    let rows = [255, 0, 0, 255, 0, 255, 0, 128, 9, 9, 9, 0, 0, 0, 0, 0];
    let file = written("answer", &png(2, 2, &rows));
    let (all, well) = answered(&WHOLE, &file);
    assert!(well);
    // Some of it is transparent, and the colors come multiplied.
    assert_eq!(all[..HEAD], head(0, 1, (2, 2), [0, 0, 2, 2], (2, 2)));
    assert_eq!(all[HEAD..], [255, 0, 0, 255, 0, 128, 0, 128, 0, 0, 0, 0, 0, 0, 0, 0]);

    // A room it does not fit, and nothing wanted whole: made smaller,
    // to the least room there is.
    let large = written("large", &png(64, 32, &[200, 100, 50, 255].repeat(64 * 32)));
    let (all, well) = answered(&["png", "1", "1", "4096", "0", "0", "0", "0", "0"], &large);
    assert!(well);
    assert_eq!(all[..HEAD], head(0, 0, (64, 32), [0, 0, 64, 32], (16, 8)));
    assert_eq!(all.len(), HEAD + 16 * 8 * 4);
    assert!(all[HEAD..].chunks(4).all(|pixel| pixel == [200, 100, 50, 255]));
    // A texture of four pixels a side takes no more, whole or not.
    let (all, _) = answered(&["png", "100", "100", "4", "8388608", "0", "0", "0", "0"], &large);
    assert_eq!(all[..HEAD], head(0, 0, (64, 32), [0, 0, 64, 32], (4, 2)));
    // A part of it at its own size: its pixels and nothing of the rest,
    // and one that reaches out of the picture is cut.
    let (all, well) = answered(&["png", "100", "100", "4096", "0", "40", "20", "30", "20"], &large);
    assert!(well);
    assert_eq!(all[..HEAD], head(0, 0, (64, 32), [40, 20, 24, 12], (24, 12)));
    assert_eq!(all.len(), HEAD + 24 * 12 * 4);
    std::fs::remove_file(file).unwrap();
    std::fs::remove_file(large).unwrap();
}

#[test]
fn it_says_why_there_is_no_picture() {
    let sound = written("sound", &png(1, 1, &[1, 2, 3, 255]));
    let none = |how: u8| head(how, 0, (0, 0), [0; 4], (0, 0));
    // What it is told is no wish: too few words, a word too many, a
    // number that is none.
    assert_eq!(answered(&WHOLE[..8], &sound).0, none(1));
    assert_eq!(answered(&[&WHOLE[..], &["0"]].concat(), &sound).0, none(1));
    assert_eq!(answered(&["png", "100", "x", "4096", "8388608", "0", "0", "0", "0"], &sound).0, none(1));
    assert_eq!(answered(&["gif", "100", "100", "4096", "8388608", "0", "0", "0", "0"], &sound).0, none(1));
    // A file that is no PNG, one of Apple's, one without data.
    let other = written("other", b"GIF89a and so on");
    assert_eq!(answered(&WHOLE, &other).0, none(3));
    let apple = written("apple", &[&b"\x89PNG\r\n\x1a\n"[..], &chunk(b"CgBI", &[0; 4])].concat());
    assert_eq!(answered(&WHOLE, &apple).0, none(5));
    let empty = written("empty", &png(1, 1, &[1, 2, 3, 255])[..8 + 25]);
    assert_eq!(answered(&WHOLE, &empty).0, none(8));
    // A file longer than is read, by its length alone: nothing of it
    // is on the disk.
    let long = written("long", b"\x89PNG\r\n\x1a\n");
    File::options().write(true).open(&long).unwrap().set_len((64 << 20) + 1).unwrap();
    assert_eq!(answered(&WHOLE, &long).0, none(7));
    for file in [sound, other, apple, empty, long] {
        std::fs::remove_file(file).unwrap();
    }
}

#[test]
fn it_sets_itself_limits_before_it_reads_and_holds_nothing_open() {
    // Its input is a pipe nobody writes to: it waits there, with its
    // limits set.
    let mut child = start(&WHOLE, Stdio::piped());
    let limits = loop {
        let limits = std::fs::read_to_string(format!("/proc/{}/limits", child.id())).unwrap();
        // The last of them to be set, and one that no process starts
        // with: a core of nothing is what many a shell hands on.
        if limits
            .lines()
            .any(|line| line.starts_with("Max open files") && line.split_whitespace().nth(3) == Some("3"))
        {
            break limits;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let limit = |name: &str| {
        let line = limits.lines().find(|line| line.starts_with(name)).unwrap();
        let mut numbers = line[name.len()..].split_whitespace();
        (numbers.next().unwrap().to_owned(), numbers.next().unwrap().to_owned())
    };
    let both = |soft: &str, hard: &str| (soft.to_owned(), hard.to_owned());
    assert_eq!(limit("Max address space"), both("268435456", "268435456"));
    assert_eq!(limit("Max cpu time"), both("10", "12"));
    assert_eq!(limit("Max file size"), both("0", "0"));
    assert_eq!(limit("Max open files"), both("3", "3"));
    assert_eq!(limit("Max core file size"), both("0", "0"));
    let mut open: Vec<String> = std::fs::read_dir(format!("/proc/{}/fd", child.id()))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    open.sort();
    assert_eq!(open, ["0", "1", "2"]);
    // Its input ends: it answers that this is no picture, and ends.
    drop(child.stdin.take());
    let mut all = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut all).unwrap();
    assert_eq!(all, head(3, 0, (0, 0), [0; 4], (0, 0)));
    assert!(ended(&mut child).success());
}

#[test]
fn it_ends_when_nobody_reads_what_it_writes() {
    // Sixteen megabytes of pixels, and a reader that goes at once: the
    // pipe takes a small part of them.
    let file = written("unread", &png(2048, 2048, &vec![7; 2048 * 2048 * 4]));
    let mut child = start(&WHOLE, File::open(&file).unwrap().into());
    drop(child.stdout.take());
    assert!(!ended(&mut child).success());
    // And an input that ends behind its first bytes ends it, and well.
    let mut child = start(&WHOLE, Stdio::piped());
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
    drop(input);
    assert!(ended(&mut child).success());
    std::fs::remove_file(file).unwrap();
}

#[test]
fn it_answers_a_jpeg_and_says_which_it_does_not_show() {
    // Sixteen by eight, red at the top and blue at the bottom, made by
    // another program, as is the progressive one beside it.
    let file = written("jpeg", include_bytes!("data/small.jpg"));
    let whole = ["jpeg", "100", "100", "4096", "8388608", "0", "0", "0", "0"];
    let (all, well) = answered(&whole, &file);
    assert!(well);
    assert_eq!(all[..HEAD], head(0, 0, (16, 8), [0, 0, 16, 8], (16, 8)));
    let pixels = &all[HEAD..];
    assert_eq!(pixels.len(), 16 * 8 * 4);
    let (first, last) = (&pixels[..4], &pixels[pixels.len() - 4..]);
    assert!(first[0] > 200 && first[2] < 60 && first[3] == 255, "{first:?}");
    assert!(last[2] > 200 && last[0] < 60 && last[3] == 255, "{last:?}");
    // A part of it is those pixels of the whole.
    let (part, well) = answered(&["jpeg", "100", "100", "4096", "0", "4", "2", "8", "4"], &file);
    assert!(well);
    assert_eq!(part[..HEAD], head(0, 0, (16, 8), [4, 2, 8, 4], (8, 4)));
    let rows: Vec<u8> = (2..6).flat_map(|y| pixels[(y * 16 + 4) * 4..(y * 16 + 12) * 4].to_vec()).collect();
    assert_eq!(part[HEAD..], rows);
    // A progressive one is turned away by name: it is not decoded.
    let progressive = written("progressive", include_bytes!("data/small-progressive.jpg"));
    assert_eq!(answered(&whole, &progressive).0, head(10, 0, (0, 0), [0; 4], (0, 0)));
    // The same bytes asked for as the other format are not that.
    assert_eq!(answered(&WHOLE, &file).0, head(3, 0, (0, 0), [0; 4], (0, 0)));
    std::fs::remove_file(file).unwrap();
    std::fs::remove_file(progressive).unwrap();
}
