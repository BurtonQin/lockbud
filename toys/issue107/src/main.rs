fn func() {
    use std::ptr;
    use std::sync::{Arc, atomic::{AtomicPtr, Ordering}};
    struct Owned<T> { data: T }
    impl<T> Owned<T> {
        fn as_raw(&self) -> *mut T { &self.data as *const _ as *mut _ }
    }
    fn opt_owned_as_raw<T>(val: &Option<Arc<Owned<T>>>) -> *mut T {
        val.as_ref().map(|o| o.as_raw()).unwrap_or(ptr::null_mut())
    }
    struct Obj<T> { ptr: AtomicPtr<T> }
    impl<T> Obj<T> {
        fn null() -> Self { Obj { ptr: AtomicPtr::new(ptr::null_mut()) } }
        fn load(&self, ord: Ordering) -> *mut T { self.ptr.load(ord) }
        fn store(&self, owned: Option<Arc<Owned<T>>>, ord: Ordering) {
            self.ptr.store(opt_owned_as_raw(&owned), ord);
        }
    }
    let o = Obj::<Vec<i32>>::null();
    let owned = Some(Arc::new(Owned { data: Vec::new() }));
    o.store(owned.clone(), Ordering::Relaxed);
    let p = o.load(Ordering::Relaxed);
    unsafe { if !p.is_null() { println!("{:?}", &*p); } }
}
fn main() { func(); }
