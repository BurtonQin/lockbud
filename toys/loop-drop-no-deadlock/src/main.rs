// The guard taken in the loop body is released on both exits, by drop() or at the
// scope end of `break`, so the lock after the loop cannot deadlock.
use std::sync::Mutex;

fn observe(m: &Mutex<u32>) -> bool {
    let mut tries = 0_u32;
    let seen = loop {
        let guard = m.lock().unwrap();
        if *guard > 3 {
            break *guard == 4;
        }
        drop(guard);
        tries += 1;
        if tries > 10 {
            break false;
        }
    };
    seen && *m.lock().unwrap() == 4
}

fn main() {
    let m = Mutex::new(std::env::args().count() as u32 + 3);
    println!("{}", observe(&m));
}
