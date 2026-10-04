//! Regression case for issue #126 (AtomicityViolation false negative).
//! The load result flows to the store through `wrapping_add` / a helper
//! call, which may stay a Call terminator in MIR (not inlined into an
//! arithmetic statement). lockbud should report an AtomicityViolation
//! for both `func` (control dep) and `func2` (data dep through a call).
use std::hint::black_box;
use std::sync::atomic::Ordering;
use std::sync::atomic::AtomicI32;

fn gen_rand_val_i32() -> i32 {
    black_box(42)
}

fn func() {
    let a = AtomicI32::new(gen_rand_val_i32());
    let v = a.load(Ordering::Relaxed); //atomic_reader
    let v3 = v.wrapping_add(1);
    let v4 = match v3 > 10 {
        true => v3.wrapping_add(2),
        false => v3.wrapping_sub(1),
    };
    if v4 > 11 && gen_rand_val_i32() < 12 {
        a.store(10, Ordering::Relaxed); //atomic_writer
    }
    println!("{:?}", a);
}

fn inc_val(v: i32) -> i32 {
    v + 1
}

fn func2() {
    let a = AtomicI32::new(gen_rand_val_i32());
    let v = a.load(Ordering::Relaxed); //atomic_reader
    let v3 = inc_val(v);
    let v4 = if v3 > 10 { v3 + 2 } else { v3 - 1 };
    if v4 > 11 && gen_rand_val_i32() < 12 {
        a.store(10, Ordering::Relaxed); //atomic_writer
    }
    println!("{:?}", a);
}

fn main() {
    func();
    func2();
}
