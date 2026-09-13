mod greeting;

fn main() {
    println!("{}", greeting::greet("world"));
    println!("{}", greeting::shout_loudly("world"));
}

fn empty() {}
