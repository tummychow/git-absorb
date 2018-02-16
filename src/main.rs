#[macro_use]
extern crate clap;
extern crate git_absorb;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;

use std::process;
use slog::Drain;

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

    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    let drain = slog::LevelFilter::new(
        drain,
        if args.is_present("verbose") {
            slog::Level::Debug
        } else {
            slog::Level::Warning
        },
    ).fuse();
    let logger = slog::Logger::root(
        drain,
        o!(
            "module" => slog::FnValue(|record| {record.module()}),
            "line" => slog::FnValue(|record| {record.line()}),
        ),
    );

    if let Err(e) = git_absorb::run(&git_absorb::Config {
        dry_run: args.is_present("dry-run"),
        force: args.is_present("force"),
        logger: &logger,
    }) {
        crit!(logger, "absorb failed"; "err" => e.description());
        // wait for async logger to finish writing messages
        drop(logger);
        process::exit(1);
    }
}
