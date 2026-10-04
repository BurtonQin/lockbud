//! Regression case for issue #130: a conflicting lock order must still be
//! detected when a guard is renamed by a move before the second lock.
//! Thread 1 takes B then A; the main thread takes A, moves the guard to a
//! new variable, then takes B. Expect one ConflictLock report.
use std::sync::{Arc, Mutex};

fn main() {
    let mut_a = Arc::new(Mutex::new(true));
    let mut_b = Arc::new(Mutex::new(true));

    let mut_a_clone = mut_a.clone();
    let mut_b_clone = mut_b.clone();

    // Thread 1: Acquires B -> A
    std::thread::spawn(move || loop {
        let _b = mut_b_clone.lock().unwrap();
        let _a = mut_a_clone.lock().unwrap();
        let _ = "thread";
    });

    // Thread 2 (Main): Acquires A -> B (Deadlock condition)
    loop {
        let _a = mut_a.lock().unwrap();
        // Moving ownership of the guard: lock still held.
        let _new_a = _a;
        let _b = mut_b.lock().unwrap();
        let _ = "main";
    }
}
