use std::sync::Arc;
use std::thread;
use std::time::Duration;
fn foo() {
    use std::sync::{Condvar, Mutex};
    struct CondPair {
        lock: Mutex<bool>,
        cvar: Condvar,
        other: Mutex<i32>,
    }
    impl CondPair {
        fn new() -> Self {
            Self {
                lock: Mutex::new(false),
                cvar: Condvar::new(),
                other: Mutex::new(1),
            }
        }
        fn wait(&self) {
            let _i = self.other.lock().unwrap();  //wait_lock
            let wait_for_condition = || {
                let mut started = self.lock.lock().unwrap();
                while !*started {
                    started = self.cvar.wait(started).unwrap();  //condvar_wait
                }
            };
            wait_for_condition();
        }
        fn notify(&self) {
            thread::sleep(Duration::from_millis(1000));
            let _i = self.other.lock().unwrap();  //notify_lock
            let mut started = self.lock.lock().unwrap();
            *started = true;
            self.cvar.notify_one();  //condvar_notify
        }
    }
    let condvar1 = Arc::new(CondPair::new());
    let condvar2 = condvar1.clone();
    let th1 = thread::spawn(move || {
        condvar1.wait();
    });
    condvar2.notify();
    th1.join().unwrap();
}
fn main() {
    foo();
}
