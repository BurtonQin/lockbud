fn panic_macro() {
    panic!("This is a panic!");
}

fn assert_panic() {
    let a = 10;
    assert_eq!(a, 12);
}

fn unwrap_panic() {
    let a: Option<i32> = None;
    let _b = a.unwrap();
}

fn expect_panic() {
    let a: Result<i32, ()> = Err(());
    let _b = a.expect("Expect panic!");
}

fn main() {
    panic_macro();
    assert_panic();
    unwrap_panic();
    expect_panic();
}
