#![feature(generators, generator_trait)]

use std::fmt::Debug;
fn gen() {
    use std::ops::{Generator, GeneratorState};
    use std::pin::Pin;
    let mut generator = || {
        yield 1;
        return "foo"
    };

    match Pin::new(&mut generator).resume(()) {
        GeneratorState::Yielded(1) => {}
        _ => panic!("unexpected value from resume"),
    }
    match Pin::new(&mut generator).resume(()) {
        GeneratorState::Complete("foo") => {}
        _ => panic!("unexpected value from resume"),
    }
}

fn recur(n: i32) -> i32 {
    if n < 2 {
        n
    } else {
        recur(n-1) + recur(n-2)
    }
}

fn level0<T: Debug>(n: i32, t: T) -> i32 {
    level1(n+1, t)
}

fn level1<T: Debug>(n: i32, t: T) -> i32 {
    level2(n+2, t)
}

fn level2<T: Debug>(n: i32, t: T) -> i32 {
    println!("{:?}", t);
    n+3
}

struct Foo {
    v: Vec<i32>,
}

impl Foo {
    fn foo(&self) -> usize {
        self.v.len()
    }
}

trait Visit {
    fn visit(&self) { println!("Trait"); }
}

impl Visit for Foo {
    fn visit(&self) { 
        self.foo();
        println!("Foo"); 
    }
}

struct Bar {
    v: i32,
}

impl Visit for Bar {
    fn visit(&self) { println!("{}, Bar", self.v); }
}

fn dynamic_dispatch(v: &dyn Visit) {
    v.visit();
}

fn static_dispatch(v: &impl Visit) {
    v.visit();
}

fn direct_visit() {
    let foo = Foo { v: Vec::new() };
    foo.visit();
}

fn static_visit() {
    let foo = Foo { v: Vec::new() };
    static_dispatch(&foo);
}

fn create_vec() -> Vec<Box<dyn Visit + 'static>> {
    let mut v: Vec<Box<dyn Visit + 'static>> = Vec::new();
    v.push(Box::new(Foo { v: Vec::new() }));
    v.push(Box::new(Bar { v: 0 }));
    v
}

fn dynamic_visit() {
    let v = create_vec();
    for vis in v {
        vis.visit();
    }
}

fn dynamic_visit2() {
    let v = create_vec();
    for vis in v {
        dynamic_dispatch(&*vis);
    }
}

fn incr(n: i32) -> i32 {
   n + 1 
}

fn fn_ptr_add() -> i32 {
    let add: fn(i32) -> i32 = incr;
    add(1)
}

fn closure_add() -> i32 {
    let a = 1;
    let add = |n: i32| n + a;
    add(1)
}

fn closure_traverse() {
    let v = Vec::new();
    let v2 = v.iter().map(|i| i+1).collect::<Vec<_>>();
    std::mem::drop(v2);
}

fn main() {
    // recur(4);
    // level0(0, "hello");
    // direct_visit();
    static_visit();
    // dynamic_visit();  // unsupported for now
    // dynamic_visit2();  // unsupported for now
    // fn_ptr_add();  // unsupported for now
    // closure_add();
    // closure_traverse();
    // gen();
}
