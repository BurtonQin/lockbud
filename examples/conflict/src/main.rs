use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

struct MyStruct {
    mu: Mutex<bool>,
    rw1: RwLock<i32>,
    rw2: RwLock<u8>,
}

impl MyStruct {
    fn new() -> Self {
        Self {
            mu: Mutex::new(true),
            rw1: RwLock::new(1),
            rw2: RwLock::new(1),
        }
    }

    fn mu_rw1(&self) -> i32 {
        let mu = self.mu.lock().unwrap();
        println!("mu_rw1: mu locked");
        thread::sleep(Duration::from_millis(1));
        let ret = match *mu {
            true => {
                let ret = self.rw1.read().unwrap();
                println!("mu_rw1: rw1 locked");
                println!("mu_rw1: rw1 unlocked");
                *ret
            },
            false => 0,
        };
        println!("mu_rw1: mu unlocked");
        ret
    }

    fn rw1_rw2(&self) -> u8 {
        let mut rw1 = self.rw1.write().unwrap();
        println!("rw1_rw2: rw1 locked");
        thread::sleep(Duration::from_millis(1));
        *rw1 += 1;
        let ret = self.rw2.read().unwrap();
        println!("rw1_rw2: rw2 locked");
        println!("rw1_rw2: rw2 unlocked");
        println!("rw1_rw2: rw1 unlocked");
        *ret
    }

    fn rw2_mu(&self) -> bool {
        let mut rw2 = self.rw2.write().unwrap();
        println!("rw2_mu: rw2 locked");
        thread::sleep(Duration::from_millis(1));
        *rw2 += 1;
        let ret = self.mu.lock().unwrap();
        println!("rw2_mu: mu locked");
        println!("rw2_mu: mu unlocked");
        println!("rw2_mu: rw2 unlocked");
        *ret
    }
}

fn main() {
    let my_struct = Arc::new(MyStruct::new());
    let clone1 = Arc::clone(&my_struct);
    let clone2 = Arc::clone(&my_struct);
    let th1 = thread::spawn(move || {
        clone1.mu_rw1();
    });
    let th2 = thread::spawn(move || {
        clone2.rw1_rw2();
    });
    my_struct.rw2_mu();
    th1.join().unwrap();
    th2.join().unwrap();
}
