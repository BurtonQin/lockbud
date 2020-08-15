use std::sync::Mutex;

trait Flyable {
    fn fly(&self);
    fn sing(&mut self) { print!("Fly"); }
    fn raise(&self, origin: i32) -> i32 {
        origin + 1
    }
}
struct Sparrow {
    name: String,
    sound: Vec<Mutex<i32>>,
}
struct Swallow {
    name: String,
    sound: String,
}

impl Flyable for Swallow {
    fn fly(&self) {
        println!("{} can fly!", self.name);
    }
    fn sing(&mut self) {
        println!("sing: {}", self.raise(1));
    }
}

impl Flyable for Sparrow {
    fn fly(&self) {
        println!("{} can fly!", self.name);
    }
    fn sing(&mut self) {
        self.append_sound(&[4,5,6]);
        println!("{:#?}", self.sound.iter().map(|x| self.raise(*x.lock().unwrap())).collect::<Vec<_>>());
    }
    fn raise(&self, origin: i32) -> i32 {
        origin + 2
    }
}

impl Sparrow {
    fn append_sound(&mut self, sound: &[i32]) {
        self.sound.extend(sound.iter().map(|s| Mutex::new(*s)));
        self.sing();
    }
}

fn foo(f: &mut Sparrow) {
    f.fly();
    f.sing();
}

fn bar(f: &mut Swallow) {
    f.fly();
    f.sing();
}

fn main() {
    let mut sparrow = Sparrow { name: "Sparrow".to_string(), sound: vec![Mutex::new(1), Mutex::new(2), Mutex::new(3)] };
    foo(&mut sparrow);
    let mut swallow = Swallow { name: "Swallo".to_string(), sound: "JiuJiu".to_string() };
    bar(&mut swallow);
}
