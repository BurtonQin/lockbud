//! Regression case for issue #110: a raw pointer into an `Rc<RefCell<_>>`
//! stays valid after the temporary `Ref` borrow guard dies at the end of
//! the statement. Dropping the borrow guard releases the borrow but does
//! not free the borrowed value, so no UseAfterFree may be reported.
use std::cell::RefCell;
use std::rc::Rc;

fn rc_and_refcell_no_uaf() {
    let data = Rc::new(RefCell::new(vec![1, 2, 3]));
    let raw_ptr = data.as_ref().borrow().as_ptr();

    unsafe {
        println!("{}", *raw_ptr);
    }
}
fn main() {
    rc_and_refcell_no_uaf();
}
