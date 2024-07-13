use std::sync::{Mutex, MutexGuard};

fn func(_g1: MutexGuard<'_, i32>, _g2: MutexGuard<'_, i32>) {
}

fn main() {
    let mu1 = Mutex::new(1);
    let mu2 = Mutex::new(1);
    loop {
        let g1 = mu1.lock().unwrap();
        let g2 = mu2.lock().unwrap();
        if *g1 == 0 {
            func(g1, g2);
            return;
        }
        std::mem::drop(g2);
        std::mem::drop(g1);
    }
}
