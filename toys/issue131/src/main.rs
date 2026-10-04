use parking_lot::RwLock;
use std::sync::Arc;

struct Foo {
    rw2: RwLock<i32>,
}

impl Foo {
    // This function causes a deadlock at runtime but is NOT detected by LockBud
    fn rwlock_read(&self) {
        (|| {
            // Acquires read lock, lifetime extends to end of match
            match *self.rw2.read() {
                1 => { 
                    // Deadlock: attempts to acquire write lock while holding read lock
                    *self.rw2.write() += 1; 
                },
                _ => {},
            };
        })()
    }
}

fn main() {
    let foo = Foo { rw2: RwLock::new(1) };
    foo.rwlock_read();
}
