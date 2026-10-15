//! The workers that answer requests for a status, for a diff, or for
//! what changed in one file.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use super::super::files::{Edition, Stamp};
use super::{Info, Line, Marks, diff, file_marks, status};

/// How many git calls run side by side. A slow repository then holds up one
/// worker, not the state of every other project.
const WORKERS: usize = 3;

#[derive(Clone, PartialEq)]
pub enum Request {
    Status(PathBuf),
    Diff(PathBuf),
    /// What changed in one file of a folder, by the path inside it, for
    /// the text that is shown of it.
    Marks(PathBuf, PathBuf, Edition),
}

impl Request {
    /// Whether two requests ask for the same thing. For which text does
    /// not tell them apart.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Marks(dir, file, _), Self::Marks(other_dir, other_file, _)) => {
                dir == other_dir && file == other_file
            }
            _ => self == other,
        }
    }
}

pub enum Reply {
    Status(PathBuf, Option<Info>),
    /// The folder, where it lies in its repository, and its changes.
    Diff(PathBuf, String, Vec<Line>),
    /// The text that was asked of, the state its file was in while git
    /// read it, or nothing if it moved meanwhile, and what changed in it.
    Marks(Edition, Option<Stamp>, Option<Marks>),
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<Request>,
    /// Being answered right now. The same request waits until that is done,
    /// so a repository that takes long occupies one worker and no more.
    running: Vec<Request>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

/// Where requests go. The workers end when this is dropped.
pub struct Requests(Arc<Shared>);

impl Requests {
    pub fn send(&self, request: Request) {
        let mut queue = self.0.queue.lock().unwrap();
        // Requests pile up while git runs, each distinct one is answered
        // once, to who asked last: marks for a text that is gone would be
        // dropped, and the text that is shown would have none.
        if let Some(waiting) = queue.waiting.iter_mut().find(|waiting| waiting.same(&request)) {
            *waiting = request;
            return;
        }
        match request {
            // A diff and a file are what someone is looking at.
            Request::Diff(_) | Request::Marks(..) => queue.waiting.push_front(request),
            Request::Status(_) => queue.waiting.push_back(request),
        }
        drop(queue);
        self.0.ready.notify_one();
    }
}

impl Drop for Requests {
    fn drop(&mut self) {
        self.0.queue.lock().unwrap().closed = true;
        self.0.ready.notify_all();
    }
}

/// Starts the workers. `reply` is called on a worker thread and returns
/// whether anyone is still listening.
pub fn spawn(reply: impl Fn(Reply) -> bool + Send + 'static) -> Requests {
    let shared = Arc::new(Shared::default());
    let reply = Arc::new(Mutex::new(reply));
    for _ in 0..WORKERS {
        let (shared, reply) = (shared.clone(), reply.clone());
        std::thread::spawn(move || work(&shared, &reply));
    }
    Requests(shared)
}

fn work(shared: &Shared, reply: &Mutex<impl Fn(Reply) -> bool>) {
    loop {
        let request = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if queue.closed {
                    return;
                }
                let free = queue.waiting.iter().position(|w| !queue.running.iter().any(|r| r.same(w)));
                if let Some(request) = free.and_then(|at| queue.waiting.remove(at)) {
                    queue.running.push(request.clone());
                    break request;
                }
                queue = shared.ready.wait(queue).unwrap();
            }
        };
        let answer = match &request {
            Request::Status(path) => Reply::Status(path.clone(), status(path)),
            Request::Diff(path) => {
                let (prefix, lines) = diff(path);
                Reply::Diff(path.clone(), prefix, lines)
            }
            Request::Marks(dir, file, of) => {
                let (stamp, marks) = file_marks(dir, file);
                Reply::Marks(*of, stamp, marks)
            }
        };
        let listening = (reply.lock().unwrap())(answer);
        let mut queue = shared.queue.lock().unwrap();
        queue.running.retain(|running| !running.same(&request));
        queue.closed |= !listening;
        drop(queue);
        // The same request may have been waiting for this one to finish.
        shared.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_request_waits_only_once() {
        let requests = Requests(Arc::new(Shared::default()));
        let path = PathBuf::from("/nowhere");
        requests.send(Request::Status(path.clone()));
        requests.send(Request::Status(path.clone()));
        requests.send(Request::Diff(path.clone()));
        // What changed in a file is asked for once for a file, and for
        // the text of it that asked last.
        let marks =
            |file: &str, change| Request::Marks(path.clone(), file.into(), Edition { opening: 1, change });
        requests.send(marks("a.rs", 1));
        requests.send(marks("a.rs", 2));
        requests.send(marks("b.rs", 1));
        let queue = requests.0.queue.lock().unwrap();
        // What someone is looking at goes first, the latest of it in front.
        assert!(
            queue.waiting
                == [
                    marks("b.rs", 1),
                    marks("a.rs", 2),
                    Request::Diff(path.clone()),
                    Request::Status(path.clone())
                ]
        );
        // While git reads a file for one text, it does not for another.
        assert!(marks("a.rs", 1).same(&marks("a.rs", 2)) && !marks("a.rs", 1).same(&marks("b.rs", 1)));
    }
}
