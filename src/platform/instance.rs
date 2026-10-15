//! A lock owns the state writer; a bounded socket hands folders to it.

use std::fs::{File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::core::config::state_dir;

const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_FOLDERS: usize = 256;
const IO_TIMEOUT: Duration = Duration::from_secs(1);
/// How long a start waits for a running instance that takes no folders:
/// one that is closing ends its sessions first, which has its own limit.
const CLOSING: Duration = Duration::from_secs(10);

pub enum Instance {
    First(Owner),
    Handed,
}

/// Kept until the event loop and the state saver have ended. Only the lock
/// holder may remove the socket, including after a failed window startup.
pub struct Owner {
    listener: UnixListener,
    socket: PathBuf,
    /// Device and inode of the socket this instance bound.
    bound: Option<(u64, u64)>,
    _lock: File,
    /// Folders are no longer taken: the app is on its way out.
    closing: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Owner {
    /// Set once the app begins to close. A start that comes after that is
    /// told so, and waits to be the next instance instead of handing its
    /// folders to one that will not open them.
    pub fn closing(&self) -> Arc<AtomicBool> {
        self.closing.clone()
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.closed.store(true, Ordering::Release);
        // Wake accept without any recurring idle timer, and without going
        // through the path: it may be gone, or another instance's by now.
        // Safety: the descriptor is the listener's, which is alive.
        unsafe { libc::shutdown(self.listener.as_raw_fd(), libc::SHUT_RDWR) };
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            let _ = worker.join();
        }
        if self.bound.is_some() && identity(&self.socket) == self.bound {
            let _ = std::fs::remove_file(&self.socket);
        }
    }
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(path).ok().map(|meta| (meta.dev(), meta.ino()))
}

/// Hands the folders to the instance behind the socket. `false` if it does
/// not take them.
fn hand(mut stream: UnixStream, message: &[u8]) -> io::Result<bool> {
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(&(message.len() as u32).to_le_bytes())?;
    stream.write_all(message)?;
    let mut ack = [0];
    stream.read_exact(&mut ack)?;
    Ok(ack == [1])
}

fn socket_path(dir: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    dir.hash(&mut hasher);
    let name = format!("codn-{:016x}.sock", hasher.finish());
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join(name),
        None => dir.join(name),
    }
}

pub fn claim(folders: &[PathBuf]) -> io::Result<Instance> {
    let dir = state_dir();
    claim_at(&dir, socket_path(&dir), folders, CLOSING)
}

fn claim_at(dir: &Path, socket: PathBuf, folders: &[PathBuf], closing: Duration) -> io::Result<Instance> {
    let lock = open_lock(dir)?;
    if !lock_or_hand(&lock, &socket, folders, closing)? {
        return Ok(Instance::Handed);
    }
    // A failed connect alone cannot prove that a socket is stale. The
    // exclusive state lock does, without racing another instance's bind,
    // for every instance that takes the lock. One that answers on the
    // socket without holding the lock is of a version before the lock, or
    // lost its state folder: starting beside it would restore and write
    // the same state twice.
    match std::fs::symlink_metadata(&socket) {
        Ok(meta) if meta.file_type().is_socket() => match connect_now(&socket) {
            Err(error)
                if matches!(error.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound) =>
            {
                std::fs::remove_file(&socket)?
            }
            _ => {
                return Err(io::Error::other(
                    "another codn answers without holding the instance lock (an older version?), close it first",
                ));
            }
        },
        Ok(_) => return Err(io::Error::other("the instance socket path is not a socket")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let at = short(&socket)?;
    let listener = UnixListener::bind(&at.path)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(Instance::First(Owner {
        listener,
        bound: identity(&socket),
        socket,
        _lock: lock,
        closing: Arc::new(AtomicBool::new(false)),
        closed: Arc::new(AtomicBool::new(false)),
        worker: Mutex::new(None),
    }))
}

fn open_lock(dir: &Path) -> io::Result<File> {
    // The user's alone from the first moment: the state and the log lie
    // here, and whoever makes the folder later makes it so as well.
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join("instance.lock"))
}

/// Takes the lock, or hands the folders to the instance that holds it.
/// `false` if a running instance took them.
fn lock_or_hand(lock: &File, socket: &Path, folders: &[PathBuf], closing: Duration) -> io::Result<bool> {
    let message = encode(folders)?;
    let deadline = Instant::now() + closing;
    // Safety: the descriptor is alive throughout this call. flock locks its
    // open file description, and releasing Owner releases the lock.
    while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }
        // An instance that is closing takes no folders, or does not answer
        // any more. This start is then the next instance, once the lock is
        // free: giving up would leave the user without a window.
        if connect_now(socket).and_then(|stream| hand(stream, &message)).is_ok_and(|taken| taken) {
            return Ok(false);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("the running instance does not take folders and does not end"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(true)
}

/// A path to a socket that fits the address of one, which holds 107
/// bytes: the path itself where it is short, and else its name in its
/// folder as the system shows the folder to this process, for as long
/// as this is kept. Without a folder for what runs, the socket lies in
/// the folder of the state, as deep as a home is.
struct Short {
    path: PathBuf,
    _folder: Option<File>,
}

fn short(socket: &Path) -> io::Result<Short> {
    let (Some(folder), Some(name)) = (socket.parent(), socket.file_name()) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid socket path"));
    };
    if socket.as_os_str().len() < 100 {
        return Ok(Short { path: socket.to_owned(), _folder: None });
    }
    let flags = libc::O_DIRECTORY | libc::O_CLOEXEC;
    let folder = OpenOptions::new().read(true).custom_flags(flags).open(folder)?;
    let path = PathBuf::from(format!("/proc/self/fd/{}", folder.as_raw_fd())).join(name);
    Ok(Short { path, _folder: Some(folder) })
}

/// Unix connect can otherwise wait indefinitely when a peer fills the
/// listener backlog. Nonblocking connect lets claim enforce its deadline.
fn connect_now(path: &Path) -> io::Result<UnixStream> {
    let at = short(path)?;
    let bytes = at.path.as_os_str().as_bytes();
    // Safety: sockaddr_un is plain initialized storage, and the copied
    // pathname is NUL-terminated with a checked bound.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid socket path"));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    if unsafe { libc::connect(fd, (&address as *const libc::sockaddr_un).cast(), length) } != 0 {
        return Err(io::Error::last_os_error());
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn encode(folders: &[PathBuf]) -> io::Result<Vec<u8>> {
    if folders.len() > MAX_FOLDERS {
        return Err(io::Error::other("too many folders"));
    }
    let mut message = Vec::new();
    for folder in folders {
        let bytes = folder.as_os_str().as_bytes();
        if bytes.len() > 4096 || message.len() + bytes.len() + 4 > MAX_MESSAGE {
            return Err(io::Error::other("folder handover is too large"));
        }
        message.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        message.extend_from_slice(bytes);
    }
    Ok(message)
}

fn decode(mut message: &[u8]) -> io::Result<Vec<PathBuf>> {
    let mut folders = Vec::new();
    while !message.is_empty() {
        if message.len() < 4 || folders.len() == MAX_FOLDERS {
            return Err(io::Error::other("invalid folder handover"));
        }
        let length = u32::from_le_bytes(message[..4].try_into().unwrap()) as usize;
        message = &message[4..];
        if length == 0 || length > 4096 || length > message.len() {
            return Err(io::Error::other("invalid folder length"));
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(message[..length].to_vec()));
        if !path.is_absolute() {
            return Err(io::Error::other("folder is not absolute"));
        }
        folders.push(path);
        message = &message[length..];
    }
    Ok(folders)
}

fn receive(stream: &mut UnixStream) -> io::Result<Vec<PathBuf>> {
    let deadline = Instant::now() + IO_TIMEOUT;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut length = [0; 4];
    read_until(stream, &mut length, deadline)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(io::Error::other("folder handover is too large"));
    }
    let mut bytes = vec![0; length];
    read_until(stream, &mut bytes, deadline)?;
    decode(&bytes)
}

fn read_until(stream: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "folder handover timed out"));
        }
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(bytes) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(n) => bytes = &mut bytes[n..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Whether the program at the other end runs as the user codn runs as.
fn own(stream: &UnixStream) -> bool {
    let mut peer = libc::ucred { pid: 0, uid: libc::uid_t::MAX, gid: 0 };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: a structure of the size that is said, which lives through
    // the call, and a descriptor that is open.
    let asked = unsafe {
        let into = std::ptr::from_mut(&mut peer).cast();
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, into, &mut length)
    };
    // SAFETY: asks for a number and changes nothing.
    asked == 0 && peer.uid == unsafe { libc::geteuid() }
}

/// The socket reader is independent of the window; stalled or oversized
/// clients never block it and never allocate an unbounded message.
pub fn listen(owner: &Owner, open: impl Fn(Vec<PathBuf>) -> bool + Send + 'static) -> io::Result<()> {
    let listener = owner.listener.try_clone()?;
    let (closing, closed) = (owner.closing.clone(), owner.closed.clone());
    let worker = std::thread::spawn(move || {
        loop {
            let accepted = listener.accept();
            if closed.load(Ordering::Acquire) {
                break;
            }
            let mut stream = match accepted {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                // Out of descriptors, for one. Not in a tight loop.
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            // Another user has no way to the socket by its permissions,
            // and is not listened to where it has one all the same.
            if !own(&stream) {
                continue;
            }
            match receive(&mut stream) {
                Ok(folders) => {
                    let accepted = !closing.load(Ordering::Acquire) && open(folders);
                    let _ = stream.write_all(&[u8::from(accepted)]);
                }
                Err(_) => {
                    let _ = stream.write_all(&[0]);
                }
            }
        }
    });
    *owner.worker.lock().unwrap() = Some(worker);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_preserves_newlines_and_rejects_bad_messages() {
        let folders = vec![PathBuf::from("/p/one\ntwo"), PathBuf::from("/p/three")];
        assert_eq!(decode(&encode(&folders).unwrap()).unwrap(), folders);
        assert!(decode(&[255; 4]).is_err());
        assert!(decode(&encode(&[PathBuf::from("relative")]).unwrap()).is_err());
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(&((MAX_MESSAGE + 1) as u32).to_le_bytes()).unwrap();
        assert!(receive(&mut reader).is_err());
    }

    #[test]
    fn a_stalled_client_has_a_deadline() {
        let (mut reader, _writer) = UnixStream::pair().unwrap();
        let begun = Instant::now();
        assert!(receive(&mut reader).is_err());
        assert!(begun.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_lock_serializes_instances_and_only_its_owner_unlinks() {
        let (dir, socket) = fixture("instance");
        let owner = first(&dir, &socket);
        let (tx, rx) = std::sync::mpsc::channel();
        listen(&owner, move |folders| tx.send(folders).is_ok()).unwrap();
        let folders = vec![PathBuf::from("/p/a\nb")];
        assert!(matches!(claim_at(&dir, socket.clone(), &folders, CLOSING).unwrap(), Instance::Handed));
        assert_eq!(rx.recv_timeout(IO_TIMEOUT).unwrap(), folders);
        assert!(socket.exists());
        drop(owner);
        assert!(!socket.exists());
        assert!(matches!(claim_at(&dir, socket, &[], CLOSING).unwrap(), Instance::First(_)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn fixture(label: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("codn-{label}-{}", crate::core::uuid()));
        (dir.clone(), dir.join("socket"))
    }

    #[test]
    fn a_socket_under_a_long_path_is_bound_and_found() {
        // The address of a socket holds 107 bytes of a path. Without a
        // folder for what runs, the socket lies in the folder of the
        // state, and a home of some length made codn not start at all,
        // with a word in its log and nowhere else.
        let (base, _) = fixture("long");
        let dir = base.join("a-folder-with-a-name-of-some-length-".repeat(4));
        let socket = dir.join("codn-0123456789abcdef.sock");
        assert!(socket.as_os_str().len() > 150);
        let owner = first(&dir, &socket);
        let (tx, rx) = std::sync::mpsc::channel();
        listen(&owner, move |folders| tx.send(folders).is_ok()).unwrap();
        let folders = vec![PathBuf::from("/p/handed")];
        assert!(matches!(claim_at(&dir, socket.clone(), &folders, CLOSING).unwrap(), Instance::Handed));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), folders);
        drop(owner);
        assert!(!socket.exists(), "and taken away under its own name");
        std::fs::remove_dir_all(base).unwrap();
    }

    /// The instance that finds nobody there.
    fn first(dir: &Path, socket: &Path) -> Owner {
        let Instance::First(owner) = claim_at(dir, socket.to_owned(), &[], CLOSING).unwrap() else {
            panic!()
        };
        owner
    }

    #[test]
    fn a_start_during_closing_waits_and_becomes_the_next_instance() {
        let (dir, socket) = fixture("closing");
        let owner = first(&dir, &socket);
        let (tx, rx) = std::sync::mpsc::channel();
        listen(&owner, move |folders| tx.send(folders).is_ok()).unwrap();
        owner.closing().store(true, Ordering::Release);
        // The closing instance ends a moment later, as after its sessions.
        let ending = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(owner);
        });
        let folders = vec![PathBuf::from("/p/kept")];
        let next = claim_at(&dir, socket.clone(), &folders, CLOSING).unwrap();
        assert!(matches!(next, Instance::First(_)), "the folders went to an instance that was closing");
        assert!(rx.try_recv().is_err());
        ending.join().unwrap();
        // One that never ends is not waited for without end.
        let begun = Instant::now();
        assert!(claim_at(&dir, socket, &[], Duration::from_millis(200)).is_err());
        assert!(begun.elapsed() < Duration::from_secs(3));
        drop(next);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_live_socket_without_the_lock_is_not_taken_over() {
        let (dir, socket) = fixture("unlocked");
        std::fs::create_dir_all(&dir).unwrap();
        // An instance of a version before the lock: it only has the socket.
        let old = UnixListener::bind(&socket).unwrap();
        assert!(claim_at(&dir, socket.clone(), &[], CLOSING).is_err());
        assert!(socket.exists() && connect_now(&socket).is_ok(), "the running instance lost its socket");
        // Once it is gone, what it left behind is stale. A program that
        // another test starts at this moment still has a copy of the
        // listener, until it has become that program.
        drop(old);
        let begun = Instant::now();
        let next = loop {
            match claim_at(&dir, socket.clone(), &[], CLOSING) {
                Err(_) if begun.elapsed() < Duration::from_secs(2) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                next => break next,
            }
        };
        assert!(matches!(next.unwrap(), Instance::First(_)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_owner_ends_without_its_path_and_leaves_another_socket_alone() {
        let (dir, socket) = fixture("pathless");
        let owner = first(&dir, &socket);
        listen(&owner, |_| true).unwrap();
        // The path is gone, and someone else's socket is in its place.
        std::fs::remove_file(&socket).unwrap();
        let other = UnixListener::bind(&socket).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(owner);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(3)).expect("the owner waits for a wake that cannot come");
        assert!(socket.exists(), "the socket of another instance was removed");
        drop(other);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
