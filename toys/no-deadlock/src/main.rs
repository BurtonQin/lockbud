use std::sync::{Mutex, MutexGuard};

fn main() {
    let mu1 = Mutex::new(1);
    let mu2 = Mutex::new(1.0);
    {
        match *mu1.lock().unwrap() {
            1 => {mu2.lock().unwrap();},
            _ => {}, 
        };
    }
    {
        match *mu2.lock().unwrap() {
            1.0 => {mu1.lock().unwrap();},
            _ => {},
        };
    }
}