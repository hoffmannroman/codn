//! The process for one picture: it is handed a file as its input and
//! told on its command line what is wanted of it, answers once on its
//! output, and ends. Before it reads a byte it gives up what it
//! inherited and sets itself limits, and it does not run where one of
//! them cannot be set.
//!
//! The limits are a bound and no cage. They stand against a decoder that
//! fails, loops or asks for memory without end. Against one that is made
//! to run foreign instructions stands that the decoders are written
//! without `unsafe`.

use std::fs::File;
use std::io::{Read, Write};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::process::ExitCode;

use super::wire::{self, FILE_MAX, Head, Wish};
use super::{jpeg, png};
use crate::core::picture::{Format, Refused};

/// What the process may ask of the system at all: the result, the sums
/// of a picture with Adam7, and the program itself.
const MEMORY: u64 = 256 << 20;
/// How long it may reckon, in seconds: over the first it is told to end,
/// at the second it is ended.
const SECONDS: (u64, u64) = (10, 12);

/// Reads the picture and says how it went, with its pixels where it has
/// some.
fn answer(wish: Option<Wish>, file: &File) -> (Head, Vec<u8>) {
    let Some(wish) = wish else { return (Head(Err(Refused::Asked)), Vec::new()) };
    // The app asked this already. Here it is asked of what was handed
    // over, and what is read stops at the limit whatever the file says.
    if file.metadata().is_ok_and(|meta| meta.len() > FILE_MAX) {
        return (Head(Err(Refused::Long)), Vec::new());
    }
    let decoded = match wish.format {
        Format::Png => png::decode(file.take(FILE_MAX), &wish),
        Format::Jpeg => jpeg::decode(file.take(FILE_MAX), &wish),
    };
    match decoded {
        // What goes out is what the app will hold it against.
        Ok(picture)
            if wish.takes(picture.decoded.size)
                && wire::bytes_of(picture.decoded.size) == Some(picture.pixels.len()) =>
        {
            (Head(Ok(picture.decoded)), picture.pixels)
        }
        Ok(_) => (Head(Err(Refused::Asked)), Vec::new()),
        Err(reason) => (Head(Err(reason)), Vec::new()),
    }
}

/// What codn does when it is started as this process, and with what it
/// ends. Called before anything else in `main`.
pub fn child() -> ExitCode {
    let limit = |what, soft: u64, hard: u64| {
        let limit = libc::rlimit { rlim_cur: soft, rlim_max: hard };
        unsafe { libc::setrlimit(what, &limit) == 0 }
    };
    unsafe {
        // What the app had open is not this process's to hold. A kernel
        // that does not know the call is asked one by one.
        let all =
            libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, 0 as libc::c_uint);
        if all != 0 {
            (3..libc::sysconf(libc::_SC_OPEN_MAX).clamp(3, 65_536) as libc::c_int).for_each(|open| {
                libc::close(open);
            });
        }
        // It goes when the app goes, also when the app could not end it.
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
            return ExitCode::FAILURE;
        }
    }
    // Without its limits it does not run. The last of them lets it hold
    // no file beside the three it has, input, output and the place for
    // faults, and the one before the last lets it make no file longer.
    // One of the three closed, it could open another: this is a bound,
    // as the head of this file says, and no cage.
    let bound = limit(libc::RLIMIT_AS, MEMORY, MEMORY)
        && limit(libc::RLIMIT_CPU, SECONDS.0, SECONDS.1)
        && limit(libc::RLIMIT_FSIZE, 0, 0)
        && limit(libc::RLIMIT_CORE, 0, 0)
        && limit(libc::RLIMIT_NOFILE, 3, 3);
    if !bound {
        return ExitCode::FAILURE;
    }
    if unsafe { libc::isatty(0) } == 1 {
        eprintln!(
            "codn: {} is how codn starts a process of its own, not for a command line",
            super::ARGUMENT
        );
        return ExitCode::FAILURE;
    }
    // Input and output as they are, with nothing held back between: the
    // pixels go out in one piece. Neither is closed here.
    let (file, mut out) =
        unsafe { (ManuallyDrop::new(File::from_raw_fd(0)), ManuallyDrop::new(File::from_raw_fd(1))) };
    let (head, pixels) = answer(Wish::read(std::env::args_os().skip(2)), &file);
    match out.write_all(&head.write()).and_then(|()| out.write_all(&pixels)) {
        Ok(()) => ExitCode::SUCCESS,
        // Nobody reads any more.
        Err(_) => ExitCode::FAILURE,
    }
}
