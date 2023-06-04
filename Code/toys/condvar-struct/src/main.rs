use std::sync::Arc;
use std::thread;

fn std_correct() {
    use std::sync::{Condvar, Mutex};

    struct CondPair {
        lock: Mutex<bool>,
        cvar: Condvar,
    }

    impl CondPair {
        fn new() -> Self {
            Self {
                lock: Mutex::new(false),
                cvar: Condvar::new(),
            }
        }
        fn wait(&self) {
            let mut started = self.lock.lock().unwrap();
            println!("start waiting!");
            while !*started {
                started = self.cvar.wait(started).unwrap();
            }
            println!("end waiting");
        }
        fn notify(&self) {
            let mut started = self.lock.lock().unwrap();
            println!("start notifying!");
            *started = true;
            self.cvar.notify_one();
            println!("end notifying!");
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

fn std_deadlock_wait() {
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
            let _i = self.other.lock().unwrap();
            let mut started = self.lock.lock().unwrap();
            println!("start waiting!");
            while !*started {
                started = self.cvar.wait(started).unwrap();
            }
            println!("end waiting");
        }
        fn notify(&self) {
            let _i = self.other.lock().unwrap();
            let mut started = self.lock.lock().unwrap();
            println!("start notifying!");
            *started = true;
            self.cvar.notify_one();
            println!("end notifying!");
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

fn std_missing_lock_before_notify() {
    use std::sync::{Condvar, Mutex};

    struct CondPair {
        lock: Mutex<bool>,
        cvar: Condvar,
    }

    impl CondPair {
        fn new() -> Self {
            Self {
                lock: Mutex::new(false),
                cvar: Condvar::new(),
            }
        }
        fn wait(&self) {
            let mut started = self.lock.lock().unwrap();
            println!("start waiting!");
            while !*started {
                started = self.cvar.wait(started).unwrap();
            }
            println!("end waiting");
        }
        fn notify(&self) {
            println!("start notifying!");
            self.cvar.notify_one();
            println!("end notifying!");
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

fn parking_lot_correct() {
    use parking_lot::{Condvar, Mutex};

    struct CondPair {
        lock: Mutex<bool>,
        cvar: Condvar,
    }

    impl CondPair {
        fn new() -> Self {
            Self {
                lock: Mutex::new(false),
                cvar: Condvar::new(),
            }
        }
        fn wait(&self) {
            let mut started = self.lock.lock();
            println!("start waiting!");
            while !*started {
                self.cvar.wait(&mut started);
            }
            println!("end waiting");
        }
        fn notify(&self) {
            let mut started = self.lock.lock();
            println!("start notifying!");
            *started = true;
            self.cvar.notify_one();
            println!("end notifying!");
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

fn parking_lot_deadlock_wait() {
    use parking_lot::{Condvar, Mutex};

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
            let _i = self.other.lock();
            let mut started = self.lock.lock();
            println!("start waiting!");
            while !*started {
                self.cvar.wait(&mut started);
            }
            println!("end waiting");
        }
        fn notify(&self) {
            let _i = self.other.lock();
            let mut started = self.lock.lock();
            println!("start notifying!");
            *started = true;
            self.cvar.notify_one();
            println!("end notifying!");
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

fn parking_lot_missing_lock_before_notify() {
    use parking_lot::{Condvar, Mutex};

    struct CondPair {
        lock: Mutex<bool>,
        cvar: Condvar,
    }

    impl CondPair {
        fn new() -> Self {
            Self {
                lock: Mutex::new(false),
                cvar: Condvar::new(),
            }
        }
        fn wait(&self) {
            let mut started = self.lock.lock();
            println!("start waiting!");
            while !*started {
                self.cvar.wait(&mut started);
            }
            println!("end waiting");
        }
        fn notify(&self) {
            println!("start notifying!");
            self.cvar.notify_one();
            println!("end notifying!");
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
    std_correct();
    std_deadlock_wait();
    std_missing_lock_before_notify();
    parking_lot_correct();
    parking_lot_deadlock_wait();
    parking_lot_missing_lock_before_notify();
}
