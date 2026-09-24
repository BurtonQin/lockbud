// Three real double locks that differ only in how the LockResult is unwrapped.
use std::sync::{Mutex, PoisonError};

struct Store {
    state: Mutex<u32>,
}

impl Store {
    fn unwrap_outer(&self) -> u32 {
        let guard = self.state.lock().unwrap();
        *guard + self.unwrap_inner()
    }
    fn unwrap_inner(&self) -> u32 {
        *self.state.lock().unwrap()
    }

    fn expect_outer(&self) -> u32 {
        let guard = self.state.lock().expect("poisoned");
        *guard + self.expect_inner()
    }
    fn expect_inner(&self) -> u32 {
        *self.state.lock().expect("poisoned")
    }

    fn tolerant_outer(&self) -> u32 {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        *guard + self.tolerant_inner()
    }
    fn tolerant_inner(&self) -> u32 {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn main() {
    let store = Store { state: Mutex::new(std::env::args().count() as u32) };
    println!("{}", store.unwrap_outer() + store.expect_outer() + store.tolerant_outer());
}
