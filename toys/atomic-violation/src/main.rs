use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicI32};

fn gen_rand_val_bool() -> bool {
    rand::random::<bool>()
}

fn gen_rand_val_i32() -> i32 {
    rand::random::<i32>()
}

fn buggy_control_dep_bool() {
    let a = AtomicBool::new(gen_rand_val_bool());
    if a.load(Ordering::Relaxed) {
        a.store(false, Ordering::Relaxed);
    }
    println!("{}", a.load(Ordering::Relaxed));
}

fn buggy_control_dep_i32() {
    let a = AtomicI32::new(gen_rand_val_i32());
    let v = a.load(Ordering::Relaxed);
    let v3 = v + 1;
    let v4 = if v3 > 10 { v3 + 2 } else { v3 - 1 };
    if v4 > 11 && gen_rand_val_i32() < 12 {
        a.store(10, Ordering::Relaxed);
    }
    println!("{:?}", a);
}

fn buggy_data_dep_i32() {
    let a = AtomicI32::new(gen_rand_val_i32());
    let v = a.load(Ordering::Relaxed);
    let v3 = v + 1;
    let v4 = if v3 > 10 { v3 + 2 } else { v3 - 1 };
    a.store(v4, Ordering::Relaxed);
    println!("{:?}", a);
}

fn buggy_both_dep_i32() {
    let a = AtomicI32::new(gen_rand_val_i32());
    let v = a.load(Ordering::Relaxed);
    let v3 = v + 1;
    let v4 = if v3 > 10 { v3 + 2 } else { v3 - 1 };
    if v4 > 11 {
        a.store(v4, Ordering::Relaxed);
    }
    println!("{:?}", a);
}

fn maybe_false_positive() {
    let a = AtomicI32::new(gen_rand_val_i32());
    let v = a.load(Ordering::Relaxed);
    let v3 = v + 1;
    let v4 = if v3 > 10 { v3 + 2 } else { v3 - 1 };
    if v4 > 11 {
        if let Ok(_v) = a.compare_exchange(v, v4, Ordering::Relaxed, Ordering::Relaxed) {
            if gen_rand_val_i32() < 12 {
                a.store(10, Ordering::Relaxed);
            }
        }
    }
    println!("{:?}", a);
}

fn main() {
    buggy_control_dep_bool();
    buggy_control_dep_i32();
    buggy_data_dep_i32();
    buggy_both_dep_i32();
    maybe_false_positive();
}
