use std::sync;
use std::thread;
use std::sync::mpsc;
use std::sync::mpsc::{Sender, Receiver};


fn main() {
    let mu1 = sync::Arc::new(sync::Mutex::new(1));
    let clone1 = mu1.clone();
    let clone2 = mu1.clone();
    let (tx, rx): (Sender<i32>, Receiver<i32>) = mpsc::channel();

    let th1 = thread::spawn(move || {
        let _r1 = clone1.lock().ok().unwrap();
        let num = rx.recv().unwrap();
        println!("{}", num);
    });


    let th2 = thread::spawn(move || {
        let _r2 = clone2.lock().ok().unwrap();
        tx.send(100).unwrap();
    });

    th1.join().unwrap();
    th2.join().unwrap();

    println!("Done!");

}