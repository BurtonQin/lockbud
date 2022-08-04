use std::sync::{Arc, Mutex, Condvar};
use std::thread;

fn test1() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair1 = pair.clone();
    let pair2 = pair.clone();

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

    println!("test1 Done!");
}

fn test2() {
    
    let mu = Arc::new(Mutex::new(1));
    let mu1 = mu.clone();
    let mu2 = mu.clone();

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair1 = pair.clone();
    let pair2 = pair.clone();

    let th1 = thread::spawn(move || {
        println!("entering th1");
        let _i = mu1.lock().unwrap();
        println!("acquring mu");
        let (lock, cvar) = &*pair1;
        let mut started = lock.lock().unwrap();
        while !*started {
            started = cvar.wait(started).unwrap();
        }
    });

    let th2 = thread::spawn(move || {
        println!("entering th2");
        let _i = mu2.lock().unwrap();
        println!("acquiring the lock");
        let (lock, cvar) = &*pair2;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();

    });
    
    th1.join().unwrap();
    th2.join().unwrap();

    println!("test1 Done!");
}


fn main() {
    test1();
    test2();
}