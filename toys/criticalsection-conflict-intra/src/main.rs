use std::sync::{Arc, Condvar, Mutex};

fn main() {
    let mu1 = Mutex::new(1);
    let cvar = Condvar::new();
    let mut unlocked = mu1.lock().unwrap();
    cvar.wait(unlocked).unwrap();
    
}
