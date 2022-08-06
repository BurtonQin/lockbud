use std::sync::{Condvar, Mutex};

fn main() {
    let mu1 = Mutex::new(1);
    let cvar = Condvar::new();
    let unlocked = mu1.lock().unwrap();
    _ = cvar.wait(unlocked).unwrap();
    
}
