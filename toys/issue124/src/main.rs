//! Regression case for issue #124: a raw pointer into a `Vec` outlives the
//! `Vec` because the owner is dropped inside a `Some(mut v)` match arm.
//! `drop(v)` lowers to a `mem::drop` call on an intermediate temporary, so
//! the manual-drop record must resolve the move chain back to `v` for the
//! use at line 20 to be reported as UseAfterFree.
fn foo() {
    fn create_obj(i: i32) -> Option<Vec<i32>> {
        if i > 10 {
            Some(vec![i])
        } else {
            None
        }
    }
    let ptr = match create_obj(11) {
        Some(mut v) => {
            let ptr = v.as_mut_ptr();
            drop(v); // free
            ptr
        }
        None => std::ptr::null_mut(),
    };
    unsafe {
        if !ptr.is_null() {
            println!("{}", *ptr); // use after free
        }
    }
}
fn main() {
    foo();
}
