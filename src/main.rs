#[macro_use]
extern crate clap;
extern crate git_absorb;

use std::process;

fn main() {
    let args = app_from_crate!().get_matches();
    println!("{:?}", args);

    if let Err(e) = git_absorb::run() {
        eprintln!("error: {:?}", e);
        process::exit(1);
    }
}
