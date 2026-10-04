//! Reproducer for issue #103, kept to document the detector's behavior.
//!
//! The reported warning is a sound over-approximation, not a false
//! positive that can be fixed without value propagation:
//!
//! ```ignore
//! let mut write_guard = self.rw.write();
//! match *write_guard {
//!     1 => { drop(write_guard); self.spin_rwlock_write_2(); }
//!     _ => {}
//! };
//! self.spin_rwlock_write_cleanup(); // re-acquires `rw`
//! ```
//!
//! `drop(write_guard)` only releases the lock on the arm that executes
//! it. On the `_` arm the guard is still held when
//! `spin_rwlock_write_cleanup` re-acquires the same lock, which would
//! deadlock at runtime. MIR keeps both paths, so lockbud reports a
//! `Possibly` DoubleLock. The program only avoids the deadlock because
//! the guarded value happens to be `1` at runtime; that fact is not
//! statically available.
use spin::RwLock;

struct Foo {
    rw: RwLock<i32>,
}
impl Foo {
    fn new() -> Self {
        Self { rw: RwLock::new(1) }
    }
    fn spin_rwlock_write_1(&self) {
        let mut write_guard = self.rw.write(); // report first lock
        match *write_guard {
            1 => {
                drop(write_guard);
                self.spin_rwlock_write_2();
            }
            _ => {}
        };
        self.spin_rwlock_write_cleanup();
    }
    fn spin_rwlock_write_cleanup(&self) {
        *self.rw.write() += 0; // reported as the second lock (possibly held twice)
    }
    fn spin_rwlock_write_2(&self) {
        *self.rw.write() += 1;
    }
}
fn main() {
    let foo1 = Foo::new();
    foo1.spin_rwlock_write_1();
    foo1.spin_rwlock_write_2();
}
