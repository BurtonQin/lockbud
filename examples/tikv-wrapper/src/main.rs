mod util;
use std::sync::{Arc, RwLock};
use util::HandyRwLock;

struct Foo {
    inner: Arc::<RwLock<i32>>,
    data: i32,
}

impl Foo {
    fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(1)),
            data: 1,
        }
    }
    fn foo(&self) {
        match *self.inner.rl() {
            1 => *self.inner.wl() += 1,
            _ => {}
        };
    }
}

fn main() {
    let f = Foo::new();
    f.foo();
    println!("Hello, world!");
}
