//! Regression case for issue #120, extracted from the `blocking` crate's
//! executor: one mutex (`inner`) is both the condvar's associated mutex
//! and locked standalone across functions, and guards are passed by value
//! into `grow_pool` from threads spawned on other threads. Before the
//! spawned-closure context fix, caller guards leaked into closures
//! running on other threads and cross-thread DoubleLock warnings were
//! reported for this pattern. Expected output: no warnings.
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

struct Inner {
    queue: VecDeque<u32>,
    idle_count: usize,
    thread_count: usize,
}

struct Executor {
    inner: Mutex<Inner>,
    cvar: Condvar,
}

fn main_loop(executor: &'static Executor) {
    let mut inner = executor.inner.lock().unwrap();
    loop {
        inner.idle_count -= 1;

        while let Some(runnable) = inner.queue.pop_front() {
            let _ = runnable;
            executor.grow_pool(inner);
            inner = executor.inner.lock().unwrap();
        }

        inner.idle_count += 1;

        let (lock, _) = executor
            .cvar
            .wait_timeout(inner, Duration::from_millis(500))
            .unwrap();
        inner = lock;

        if inner.queue.is_empty() && inner.idle_count > 2 {
            inner.idle_count -= 1;
            inner.thread_count -= 1;
            break;
        }
    }
}

fn schedule(executor: &'static Executor, runnable: u32) {
    let mut inner = executor.inner.lock().unwrap();
    inner.queue.push_back(runnable);
    executor.cvar.notify_one();
    executor.grow_pool(inner);
}

impl Executor {
    fn grow_pool(&'static self, mut inner: MutexGuard<'static, Inner>) {
        let overload = !inner.queue.is_empty() && inner.idle_count == 0;
        if overload {
            inner.idle_count += 1;
            inner.thread_count += 1;
            thread::spawn(move || main_loop(self));
        }
    }
}

fn main() {
    let executor: &'static Executor = Box::leak(Box::new(Executor {
        inner: Mutex::new(Inner {
            queue: VecDeque::new(),
            idle_count: 0,
            thread_count: 0,
        }),
        cvar: Condvar::new(),
    }));
    thread::spawn(move || main_loop(executor));
    schedule(executor, 1);
    schedule(executor, 2);
}
