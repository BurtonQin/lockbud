//! Regression case for issue #129: moving a guard into a new variable
//! (`let _new_tmp2 = _tmp2;`) is an ownership transfer, not another lock
//! acquisition. lockbud must report exactly one DoubleLock here, pointing
//! at the second `lock()` call (line 10), not at the move (line 11).
use std::sync::Mutex;

fn main() {
    let lock = Mutex::new(false);
    let _tmp = lock.lock().unwrap();
    let _tmp2 = lock.lock().unwrap(); // the real DoubleLock
    let _new_tmp2 = _tmp2; // must not be reported as a third acquisition
}
