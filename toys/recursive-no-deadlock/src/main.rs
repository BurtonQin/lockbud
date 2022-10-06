use parking_lot;

fn parking_lot_rwlock() -> i32 {
    let rw1 = parking_lot::RwLock::new(1);
    let mut a = 0;
    match *rw1.read() {
        1 => { a = *rw1.read_recursive() + a; },
        _ => { a = *rw1.read() + a; },
    };
    a
}

fn main() {
    parking_lot_rwlock();
}
