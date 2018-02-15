extern crate git_absorb;

use std::env;
use std::process;

fn main() {
    if let Err(e) = git_absorb::run() {
        eprintln!("error: {:?}", e);
        process::exit(1);
    }

    let args: Vec<String> = env::args().collect();
    println!("{:?}", args);
}
