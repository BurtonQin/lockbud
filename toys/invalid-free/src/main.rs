use std::mem;
use std::ptr::addr_of_mut;

fn uninit() {
    unsafe {
        #[allow(invalid_value, deprecated)]
        let _obj: Vec<i32> = mem::uninitialized();
    }
}

fn assume_write_fp() {
    let mut uninit = std::mem::MaybeUninit::<Vec<i32>>::uninit();
    unsafe {
        uninit.write(Vec::new());
        uninit.assume_init();   
    }
}

fn assume_ptr_write_fp() {
    #[derive(Debug)]
    struct Obj {
        a: Vec<i32>,
        b: bool,
    }

    let mut uninit = std::mem::MaybeUninit::<Obj>::uninit();
    unsafe {
        let ptr = uninit.as_mut_ptr();
        addr_of_mut!((*ptr).a).write(Vec::new());
        addr_of_mut!((*ptr).b).write(true);
        uninit.assume_init();
    }
}

fn assume() {
    let uninit = std::mem::MaybeUninit::<Vec<i32>>::uninit();
    unsafe {
        uninit.assume_init();   
    }    
}

fn main() {
    assume_write_fp();
    assume_ptr_write_fp();
    assume();
    uninit();
}
