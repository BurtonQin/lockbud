//! Regression case for issue #106: two macro invocations locking different
//! Arc-shared mutexes, one in the caller and one in a spawned thread. The
//! macro's `$a`/`$b` spans repeat in both expansions, but no guard of the
//! caller is captured by the spawned closure, so no DoubleLock may be
//! reported for it. (The reversed lock order across the two threads is a
//! ConflictLock, which lockbud does report.)
use std::sync::{Arc, Mutex};
use std::thread;

macro_rules! lock_and_execute {
    ($a:expr, $b:expr, $body:block) => {
        let _a = $a.lock().unwrap();
        let _b = $b.lock().unwrap();
        $body
    };
}
fn func() {
    let lock_a1 = Arc::new(Mutex::new(1));
    let lock_b1 = Arc::new(Mutex::new(true));
    let lock_a2 = lock_a1.clone();
    let lock_b2 = lock_b1.clone();
    lock_and_execute!(lock_b1, lock_a1, {});
    let th = thread::spawn(move || {
        lock_and_execute!(lock_a2, lock_b2, {});
    });
    th.join().unwrap();
}
fn main() {
    func();
}
