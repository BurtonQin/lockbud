use std::sync::{Arc, Mutex};

fn main() {
    let mut_a = Arc::new(Mutex::new(true));
    let mut_b = Arc::new(Mutex::new(true));

    let mut_a_clone = mut_a.clone();
    let mut_b_clone = mut_b.clone();
    std::thread::spawn(move || loop {
        let _b = mut_b_clone.lock().unwrap();
        let _a = mut_a_clone.lock().unwrap();
        dbg!("thread");
    });

    loop {
        let _a = mut_a.lock().unwrap();
        let _b = mut_b.lock().unwrap();
        dbg!("main");
    }
}
