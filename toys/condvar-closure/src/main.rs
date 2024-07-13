use std::sync::Arc;
use std::thread;

fn std_correct() {
    use std::sync::{Condvar, Mutex};

    let pair1 = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair1.clone();

    let th1 = thread::spawn(move || {
        let (lock, cvar) = &*pair1;
        let mut started = lock.lock().unwrap();
        while !*started {
            started = cvar.wait(started).unwrap();
        }
    });

    let th2 = thread::spawn(move || {
        let (lock, cvar) = &*pair2;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();
    });

    th1.join().unwrap();
    th2.join().unwrap();
}

fn std_deadlock_wait() {
    use std::sync::{Condvar, Mutex};
    let mu1 = Arc::new(Mutex::new(1));
    let mu2 = mu1.clone();

    let pair1 = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair1.clone();

    let th1 = thread::spawn(move || {
        let _i = mu1.lock().unwrap();
        let (lock, cvar) = &*pair1;
        let mut started = lock.lock().unwrap();
        while !*started {
            started = cvar.wait(started).unwrap();
        }
    });

    let th2 = thread::spawn(move || {
        let _i = mu2.lock().unwrap();
        let (lock, cvar) = &*pair2;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();
    });

    th1.join().unwrap();
    th2.join().unwrap();
}

fn std_missing_lock_before_notify() {
    use std::sync::{Condvar, Mutex};

    let pair1 = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair1.clone();

    let th1 = thread::spawn(move || {
        let (lock, cvar) = &*pair1;
        let mut started = lock.lock().unwrap();
        while !*started {
            started = cvar.wait(started).unwrap();
        }
    });

    let th2 = thread::spawn(move || {
        let (_, cvar) = &*pair2;
        cvar.notify_one();
    });

    th1.join().unwrap();
    th2.join().unwrap();
}

fn parking_lot_correct() {
    use parking_lot::{Condvar, Mutex};

    let pair1 = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair1.clone();

    let th1 = thread::spawn(move || {
        let (lock, cvar) = &*pair1;
        let mut started = lock.lock();
        while !*started {
            cvar.wait(&mut started);
        }
    });

    let th2 = thread::spawn(move || {
        let (lock, cvar) = &*pair2;
        let mut started = lock.lock();
        *started = true;
        cvar.notify_one();
    });

    th1.join().unwrap();
    th2.join().unwrap();
}

fn parking_lot_deadlock_wait() {
    use parking_lot::{Condvar, Mutex};
    let mu1 = Arc::new(Mutex::new(1));
    let mu2 = mu1.clone();

    let pair1 = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair1.clone();

    let th1 = thread::spawn(move || {
        let _i = mu1.lock();
        let (lock, cvar) = &*pair1;
        let mut started = lock.lock();
        while !*started {
            cvar.wait(&mut started);
        }
    });

    let th2 = thread::spawn(move || {
        let _i = mu2.lock();
        let (lock, cvar) = &*pair2;
        let mut started = lock.lock();
        *started = true;
        cvar.notify_one();
    });

    th1.join().unwrap();
    th2.join().unwrap();
}

fn parking_lot_missing_lock_before_notify() {
    use parking_lot::{Condvar, Mutex};

    let pair1 = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair1.clone();

    let th1 = thread::spawn(move || {
        let (lock, cvar) = &*pair1;
        let mut started = lock.lock();
        while !*started {
            cvar.wait(&mut started);
        }
    });

    let th2 = thread::spawn(move || {
        let (_lock, cvar) = &*pair2;
        cvar.notify_one();
    });

    th1.join().unwrap();
    th2.join().unwrap();
}

fn main() {
    std_correct();
    std_deadlock_wait();
    std_missing_lock_before_notify();
    parking_lot_correct();
    parking_lot_deadlock_wait();
    parking_lot_missing_lock_before_notify();
}
