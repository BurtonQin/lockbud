fn foo() -> Box<dyn Fn() -> i32> {
    let data = vec![1, 2, 3];
    let raw_ptr = data.as_ptr();
    let closure = move || { unsafe { *raw_ptr } };
    let result = closure();
    println!("R1: {}", result);
    Box::new(closure)
}
fn main() {
    let closure = foo();
    let result = closure();
    println!("R2: {}", result);
}
