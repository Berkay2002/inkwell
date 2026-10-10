//! A small priority worker pool.
//!
//! Jobs are picked by `(class, newest epoch first, rank)`. The epoch and rank live in atomics so a
//! queued job can be re-prioritised when the same work is requested again (scrolling back).

use parking_lot::{Condvar, Mutex};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Background jobs allowed to run at once. They all end up serialised on their document's lock,
/// so more than one would only park workers that visible tiles could use.
const MAX_BACKGROUND: usize = 1;

/// Job classes, lowest runs first.
pub mod class {
    /// Tiles the user is looking at.
    pub const VISIBLE: u8 = 0;
    /// Text layer, outline: needed soon, cheap.
    pub const INTERACTIVE: u8 = 1;
    /// Search, page-size scan: must not starve rendering.
    pub const BACKGROUND: u8 = 2;
}

/// Where a job sits in the queue. All of it may change while the job waits (the class only ever
/// goes down: a main-view request joining a thumbnail job).
#[derive(Default)]
pub struct Prio {
    pub class: AtomicU8,
    pub epoch: AtomicU64,
    pub rank: AtomicU32,
}

impl Prio {
    pub fn new(class: u8, epoch: u64, rank: u32) -> Arc<Self> {
        Arc::new(Self { class: AtomicU8::new(class), epoch: AtomicU64::new(epoch), rank: AtomicU32::new(rank) })
    }
}

pub struct Job {
    pub prio: Arc<Prio>,
    pub run: Box<dyn FnOnce() + Send>,
}

struct Shared {
    queue: Mutex<Vec<Job>>,
    cv: Condvar,
    stop: AtomicBool,
    /// Changed only while holding `queue`, so a waiting worker cannot miss the wake-up.
    bg_running: AtomicUsize,
}

pub struct Pool {
    shared: Arc<Shared>,
}

impl Pool {
    pub fn new(threads: usize) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Vec::new()),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            bg_running: AtomicUsize::new(0),
        });
        for i in 0..threads {
            let s = shared.clone();
            std::thread::Builder::new()
                .name(format!("inkwell-worker-{i}"))
                .spawn(move || worker(s))
                .expect("spawn worker");
        }
        Self { shared }
    }

    pub fn submit(&self, job: Job) {
        self.shared.queue.lock().push(job);
        self.shared.cv.notify_one();
    }

    pub fn pending(&self) -> usize {
        self.shared.queue.lock().len()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.cv.notify_all();
    }
}

fn worker(s: Arc<Shared>) {
    loop {
        let (job, background) = {
            let mut q = s.queue.lock();
            loop {
                if s.stop.load(Ordering::SeqCst) {
                    return;
                }
                let bg_full = s.bg_running.load(Ordering::SeqCst) >= MAX_BACKGROUND;
                if let Some(i) = best(&q, bg_full) {
                    let job = q.swap_remove(i);
                    let background = job.prio.class.load(Ordering::SeqCst) == class::BACKGROUND;
                    if background {
                        s.bg_running.fetch_add(1, Ordering::SeqCst);
                    }
                    break (job, background);
                }
                s.cv.wait(&mut q);
            }
        };
        // A bad PDF must never take a worker down.
        let _ = catch_unwind(AssertUnwindSafe(job.run));
        if background {
            {
                let _q = s.queue.lock();
                s.bg_running.fetch_sub(1, Ordering::SeqCst);
            }
            s.cv.notify_all();
        }
    }
}

fn best(q: &[Job], bg_full: bool) -> Option<usize> {
    q.iter()
        .enumerate()
        .filter(|(_, j)| !(bg_full && j.prio.class.load(Ordering::Relaxed) == class::BACKGROUND))
        .min_by_key(|(_, j)| {
            (
                j.prio.class.load(Ordering::Relaxed),
                std::cmp::Reverse(j.prio.epoch.load(Ordering::Relaxed)),
                j.prio.rank.load(Ordering::Relaxed),
            )
        })
        .map(|(i, _)| i)
}
