use std::sync;

struct Foo {
    mu1: sync::Arc<sync::Mutex<i32>>,
    rw1: sync::RwLock<i32>,
}

impl Foo {
    fn new() -> Self {
        Self {
            mu1: sync::Arc::new(sync::Mutex::new(1)),
            rw1: sync::RwLock::new(1),
        }
    }

    fn std_mutex_1(&self) {
        match *self.mu1.lock().unwrap() {
            1 => {},
            _ => { self.std_rw_2(); },
        };
    }

    fn std_rw_2(&self) {
        *self.rw1.write().unwrap() += 1;
    }

    fn std_rw_1(&self) {
        match *self.rw1.read().unwrap() {
            1 => {},
            _ => { self.std_mutex_2(); },
        }
    }

    fn std_mutex_2(&self) {
        *self.mu1.lock().unwrap() += 1;
    }
}

fn main() {}
