//! Regression case for issue #121: passing a lock guard directly as a
//! by-value parameter must not produce a DoubleLock false positive. The
//! MIR ABI passes the argument as `copy` while the caller keeps dropping
//! it on unwind; the guard dies on the normal-return edge, so re-locking
//! afterwards is safe.
use std::sync::Mutex;

fn take(g: std::sync::MutexGuard<'_, i32>) {
    println!("{}", *g);
}

fn main() {
    let m = Mutex::new(1);
    let g = m.lock().unwrap();
    take(g);
    let g2 = m.lock().unwrap();
    println!("{}", *g2);
}
