//! What cutting the lines of real files into pieces costs: time by
//! language and by the length of a line, the first use of a language,
//! and the memory, counted at the allocator. One thread, as the process
//! of codn would have.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Counts what is held on the heap. Two additions for every allocation,
/// which the times below include.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn took(bytes: usize) {
    PEAK.fetch_max(LIVE.fetch_add(bytes, Relaxed) + bytes, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        took(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(at, layout) }
    }

    unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size(), Relaxed);
        took(size);
        unsafe { System.realloc(at, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Endings the grammars do not name themselves, as codn is to map them.
const ALSO: [(&str, &str); 8] = [
    ("jsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("jsonl", "json"),
    ("jsonc", "json"),
    ("astro", "html"),
    ("mdx", "md"),
    ("mts", "ts"),
];

/// A line over this is not cut at all here: it would be the whole run.
const SKIPPED: usize = 1 << 20;
/// How far apart the places to go on from lie, in lines.
const EVERY: usize = 64;
/// A line of no more than `SHORT` bytes that takes this long has met a
/// search that gave up: the engine ends one after a million steps back,
/// and the crate takes that for "no match", so its colors may be wrong.
const GAVE_UP: Duration = Duration::from_millis(5);
const SHORT: usize = 4 << 10;
/// The classes of lines by their length in bytes, each up to its number.
const CLASSES: [usize; 6] = [256, 1 << 10, 4 << 10, 16 << 10, 64 << 10, usize::MAX];

#[derive(Clone, Copy, Default)]
struct Class {
    lines: usize,
    time: Duration,
    slowest: Duration,
}

#[derive(Default)]
struct Language {
    files: usize,
    bytes: usize,
    lines: usize,
    /// The second cutting of every file: the patterns it needs are there.
    steady: Duration,
    /// What the first cutting of each file took more than its second:
    /// the patterns it was the first to need. All of them together are
    /// every pattern the files of the language ever needed.
    first: Vec<Duration>,
    /// The file that took longest: its time, its lines, its bytes.
    longest: (Duration, usize, usize),
    /// What the heap held more after its files than before.
    held: isize,
    classes: [Class; CLASSES.len()],
    skipped: usize,
    /// Lines the crate could not cut or whose pieces do not nest, and
    /// where and why the first of them.
    failed: usize,
    failure: Option<String>,
    /// Short lines that took as long as a search that gives up.
    gave_up: usize,
    /// Places to go on from: how many, their bytes, the largest.
    places: (usize, usize, usize),
    /// The slowest line: its time, its bytes, where.
    slowest: (Duration, usize, String),
}

fn grammar<'a>(set: &'a SyntaxSet, path: &Path) -> Option<&'a SyntaxReference> {
    let ending = path.extension().and_then(|ending| ending.to_str()).map(str::to_lowercase);
    let name = path.file_name().and_then(|name| name.to_str());
    let by = |ending: &str| {
        let ending = ALSO.iter().find(|(also, _)| *also == ending).map_or(ending, |(_, known)| known);
        set.find_syntax_by_extension(ending)
    };
    ending.as_deref().and_then(by).or_else(|| name.and_then(by))
}

/// Cuts a text into pieces once and returns the time of it. With
/// `language`, what the lines cost is written down there.
fn cut(
    set: &SyntaxSet,
    syntax: &SyntaxReference,
    text: &str,
    at: &str,
    mut language: Option<&mut Language>,
) -> Duration {
    let (mut state, mut stack) = (ParseState::new(syntax), ScopeStack::new());
    let mut whole = Duration::ZERO;
    for (number, line) in text.split_inclusive('\n').enumerate() {
        if line.len() > SKIPPED {
            if let Some(language) = language.as_deref_mut() {
                language.skipped += 1;
            }
            continue;
        }
        let begun = Instant::now();
        let pieces = state.parse_line(line, set);
        let mut failed = pieces.as_ref().err().map(ToString::to_string);
        for (_, change) in pieces.iter().flatten() {
            if let Err(error) = stack.apply(change) {
                failed.get_or_insert(error.to_string());
            }
        }
        let time = begun.elapsed();
        whole += time;
        let Some(language) = language.as_deref_mut() else { continue };
        if let Some(why) = failed {
            language.failed += 1;
            language.failure.get_or_insert_with(|| format!("{at}:{}: {why}", number + 1));
        }
        language.gave_up += (line.len() <= SHORT && time >= GAVE_UP) as usize;
        let class = CLASSES.iter().position(|limit| line.len() <= *limit).unwrap_or(CLASSES.len() - 1);
        let class = &mut language.classes[class];
        (class.lines, class.time, class.slowest) =
            (class.lines + 1, class.time + time, class.slowest.max(time));
        if time > language.slowest.0 {
            language.slowest = (time, line.len(), format!("{at}:{}", number + 1));
        }
        if number % EVERY == EVERY - 1 {
            let before = LIVE.load(Relaxed);
            let place = (state.clone(), stack.clone());
            let bytes = LIVE.load(Relaxed).saturating_sub(before) + std::mem::size_of_val(&place);
            language.places =
                (language.places.0 + 1, language.places.1 + bytes, language.places.2.max(bytes));
        }
    }
    whole
}

fn ms(time: Duration) -> String {
    format!("{:.1}", time.as_secs_f64() * 1000.0)
}

fn mib(bytes: usize) -> String {
    format!("{:.2}", bytes as f64 / 1_048_576.0)
}

pub fn run(packed: &Path, list: &Path) -> Result<(), String> {
    let data = std::fs::read(packed).map_err(|e| format!("{}: {e}", packed.display()))?;
    let (before, begun) = (LIVE.load(Relaxed), Instant::now());
    let set: SyntaxSet =
        syntect::dumps::from_uncompressed_data(&data).map_err(|e| format!("{}: {e}", packed.display()))?;
    let loaded = begun.elapsed();
    let grammars = LIVE.load(Relaxed) - before;
    println!(
        "grammars: {} of them in {} MiB, read in {} ms into {} MiB",
        set.syntaxes().len(),
        mib(data.len()),
        ms(loaded),
        mib(grammars)
    );
    drop(data);

    // The files of one language after one another, so that what the heap
    // holds more after them is what that language made it hold.
    let list = std::fs::read_to_string(list).map_err(|e| format!("{}: {e}", list.display()))?;
    let mut files: Vec<(&str, &str)> = Vec::new();
    let (mut unknown, mut unread) = (0, 0);
    for path in list.lines() {
        match grammar(&set, Path::new(path)) {
            Some(syntax) => files.push((syntax.name.as_str(), path)),
            None => unknown += 1,
        }
    }
    files.sort();

    let mut languages: BTreeMap<&str, Language> = BTreeMap::new();
    let mut live = LIVE.load(Relaxed) as isize;
    for (at, (name, path)) in files.iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(path) else {
            unread += 1;
            continue;
        };
        let syntax = set.find_syntax_by_name(name).ok_or("a grammar went missing")?;
        let language = languages.entry(name).or_default();
        let first = cut(&set, syntax, &text, path, None);
        let again = cut(&set, syntax, &text, path, Some(language));
        let lines = text.split_inclusive('\n').count();
        language.files += 1;
        language.bytes += text.len();
        language.lines += lines;
        language.steady += again;
        language.first.push(first.saturating_sub(again));
        language.longest = language.longest.max((again, lines, text.len()));
        drop(text);
        if files.get(at + 1).is_none_or(|(next, _)| next != name) {
            let now = LIVE.load(Relaxed) as isize;
            (language.held, live) = (now - live, now);
        }
    }

    println!(
        "\nlanguage\tfiles\tlines\tMiB\tlines/s\tMiB/s\tpatterns ms: all\tmost for a file\theld MiB\t\
         longest file ms\tits lines\tplaces\tmean B\tlargest B\tskipped\tfailed\tgave up\tslowest line ms\tits bytes"
    );
    for (name, l) in &mut languages {
        let seconds = l.steady.as_secs_f64().max(1e-9);
        l.first.sort_unstable();
        println!(
            "{name}\t{}\t{}\t{}\t{:.0}\t{:.2}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            l.files,
            l.lines,
            mib(l.bytes),
            l.lines as f64 / seconds,
            l.bytes as f64 / 1_048_576.0 / seconds,
            ms(l.first.iter().sum()),
            ms(l.first.last().copied().unwrap_or_default()),
            mib(l.held.max(0) as usize),
            ms(l.longest.0),
            l.longest.1,
            l.places.0,
            l.places.1.checked_div(l.places.0).unwrap_or(0),
            l.places.2,
            l.skipped,
            l.failed,
            l.gave_up,
            ms(l.slowest.0),
            l.slowest.1,
        );
    }

    println!(
        "\nlines by their length, all languages\nup to bytes\tlines\tms\tµs a line\tµs a KiB\tslowest ms"
    );
    let mut all = [Class::default(); CLASSES.len()];
    let mut bytes = [0usize; CLASSES.len()];
    for language in languages.values() {
        for (class, of) in all.iter_mut().zip(&language.classes) {
            (class.lines, class.time, class.slowest) =
                (class.lines + of.lines, class.time + of.time, class.slowest.max(of.slowest));
        }
    }
    // The bytes of a class are not kept by language: read once more, cheaply.
    for (_, path) in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for line in text.split_inclusive('\n').filter(|line| line.len() <= SKIPPED) {
            bytes[CLASSES.iter().position(|limit| line.len() <= *limit).unwrap_or(CLASSES.len() - 1)] +=
                line.len();
        }
    }
    for ((limit, class), bytes) in CLASSES.iter().zip(&all).zip(&bytes) {
        let micros = class.time.as_secs_f64() * 1e6;
        let limit = if *limit == usize::MAX { "more".into() } else { limit.to_string() };
        println!(
            "{limit}\t{}\t{}\t{:.1}\t{:.1}\t{}",
            class.lines,
            ms(class.time),
            micros / class.lines.max(1) as f64,
            micros / (*bytes as f64 / 1024.0).max(1e-9),
            ms(class.slowest)
        );
    }

    println!("\nthe slowest line of each language");
    for (name, language) in &languages {
        println!(
            "{name}\t{} ms\t{} bytes\t{}",
            ms(language.slowest.0),
            language.slowest.1,
            language.slowest.2
        );
    }
    for (name, language) in &languages {
        if let Some(failure) = &language.failure {
            println!("\n{name} failed first at {failure}");
        }
    }
    // What the system counts for the process, beside what the allocator
    // was asked for: now, and at its most.
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let of = |key: &str| {
        let line = status.lines().find(|line| line.starts_with(key));
        line.and_then(|line| line.split_whitespace().nth(1)).unwrap_or("?").to_owned()
    };
    println!(
        "\nheap at the end {} MiB, at its most {} MiB; resident {} kB, at its most {} kB; \
         {unknown} files without a grammar, {unread} not read as text",
        mib(LIVE.load(Relaxed)),
        mib(PEAK.load(Relaxed)),
        of("VmRSS:"),
        of("VmHWM:"),
    );
    Ok(())
}
