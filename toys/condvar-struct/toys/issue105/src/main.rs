use parking_lot::RwLock;
use std::sync;
fn parking_lot_rwlock() -> i32 {
    let rw1 = RwLock::new(1);
    fn read_block<R: std::ops::Deref<Target = i32>>(read_val: R, lock: &parking_lot::RwLock<i32>) {
        if *read_val == 1 {
            *lock.write() += 1; // line 7: not released
        }
    }
    let read_scoped = rw1.read();  // line 10: should report a double lock warning
    read_block(read_scoped, &rw1);
    0
}
fn main() {
    parking_lot_rwlock();
}
