use std::sync::Mutex;
use std::sync::MutexGuard;

fn wait<T>(a: MutexGuard<T>) -> MutexGuard<T> {
    a
}

fn main() {
    let mu = Mutex::new(0);
    let mut lg = mu.lock().unwrap();
    lg = wait(lg);
    std::mem::drop(lg);
}
