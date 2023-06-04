use std::sync::Mutex;
use std::sync::Arc;
#[macro_use]
extern crate lazy_static;

lazy_static! {
    static ref GRAPH_ACQUIRE_LOCK: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
}

fn main() {
    let _tmp = GRAPH_ACQUIRE_LOCK.lock().unwrap();
    let _tmp2 = GRAPH_ACQUIRE_LOCK.lock().unwrap();
}
