fn drop_in_match() {
    fn create_obj(i: i32) -> Option<Vec<i32>> {
        if i > 10 {
            Some(Vec::new())
        } else {
            None
        }
    }
    let ptr = match create_obj(11) {
        Some(mut v) => v.as_mut_ptr(),
        None => std::ptr::null_mut(),
    };
    unsafe {
        if !ptr.is_null() {
            println!("{}", *ptr);
        }
    }
}
fn escape_to_param() {
    use std::ptr;
    use std::sync::atomic::{AtomicPtr, Ordering};
    struct Owned<T> {
        data: T,
    }
    impl<T> Owned<T> {
        fn as_raw(&self) -> *mut T {
            &self.data as *const _ as *mut _
        }
    }
    fn opt_owned_as_raw<T>(val: &Option<Owned<T>>) -> *mut T {
        val.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut())
    }
    struct Obj<T> {
        ptr: AtomicPtr<T>,
    }
    impl<T> Obj<T> {
        fn null() -> Self {
            Obj {
                ptr: AtomicPtr::new(ptr::null_mut()),
            }
        }
        fn load(&self, ord: Ordering) -> *mut T {
            self.ptr.load(ord)
        }
        fn store(&self, owned: Option<Owned<T>>, ord: Ordering) {
            self.ptr.store(opt_owned_as_raw(&owned), ord);
        }
    }
    let o = Obj::<Vec<i32>>::null();
    let owned = Some(Owned { data: Vec::new() });
    o.store(owned, Ordering::Relaxed);
    let p = o.load(Ordering::Relaxed);
    unsafe {
        println!("{:?}", *p);
    }
}

fn escape_to_global() {
    use std::os::raw::{c_char, c_int};
    use std::ptr;
    #[repr(C)]
    pub struct hostent {
        h_name: *mut c_char,
        h_aliases: *mut *mut c_char,
        h_addrtype: c_int,
        h_length: c_int,
        h_addr_list: *mut *mut c_char,
    }

    static mut HOST_ENTRY: hostent = hostent {
        h_name: ptr::null_mut(),
        h_aliases: ptr::null_mut(),
        h_addrtype: 0,
        h_length: 0,
        h_addr_list: ptr::null_mut(),
    };

    static mut HOST_NAME: Option<Vec<u8>> = None;
    static mut HOST_ALIASES: Option<Vec<Vec<u8>>> = None;

    pub unsafe extern "C" fn gethostent() -> *const hostent {
        HOST_ALIASES = Some(vec![vec![0, 1, 2], vec![3, 4, 5]]);
        let mut host_aliases: Vec<*mut i8> = HOST_ALIASES
            .as_mut()
            .unwrap()
            .iter_mut()
            .map(|x| x.as_mut_ptr() as *mut i8)
            .collect();
        host_aliases.push(ptr::null_mut());
        host_aliases.push(ptr::null_mut());

        HOST_NAME = Some(vec![0, 1, 2]);

        HOST_ENTRY = hostent {
            h_name: HOST_NAME.as_mut().unwrap().as_mut_ptr() as *mut c_char,
            h_aliases: host_aliases.as_mut_slice().as_mut_ptr() as *mut *mut i8,
            h_addrtype: 0,
            h_length: 4,
            h_addr_list: ptr::null_mut(),
        };
        &HOST_ENTRY as *const hostent
    }

    unsafe {
        let h = gethostent();
        println!("{:?}", *(&*h).h_aliases);
    }
}

fn main() {
    drop_in_match();
    escape_to_param();
    escape_to_global();
}
