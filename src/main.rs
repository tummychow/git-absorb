#[macro_use]
extern crate clap;
extern crate git_absorb;

use std::process;

fn main() {
    let args = app_from_crate!()
        .about("Automatically absorb staged changes into your current branch")
        .arg(
            clap::Arg::with_name("dry-run")
                .help("Don't make any actual changes")
                .short("n")
                .long("dry-run")
                .takes_value(false),
        )
        .arg(
            clap::Arg::with_name("force")
                .help("Skip safety checks")
                .short("f")
                .long("force")
                .takes_value(false),
        )
        .arg(
            clap::Arg::with_name("verbose")
                .help("Display more output")
                .short("v")
                .long("verbose")
                .takes_value(false),
        )
        .get_matches();
    println!("{:?}", args);

    if let Err(e) = git_absorb::run(&git_absorb::Config {
        dry_run: args.is_present("dry-run"),
        force: args.is_present("force"),
        verbose: args.is_present("verbose"),
    }) {
        eprintln!("error: {:?}", e);
        process::exit(1);
    }
}
