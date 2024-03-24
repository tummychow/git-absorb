#[macro_use]
extern crate clap;

#[macro_use]
extern crate slog;

use clap::ArgAction;
use clap_complete::{generate, Shell};
use slog::Drain;
use std::io;

fn main() {
    let args = command!()
        .about("Automatically absorb staged changes into your current branch")
        .arg(
            clap::Arg::new("base")
                .help("Use this commit as the base of the absorb stack")
                .short('b')
                .long("base"),
        )
        .arg(
            clap::Arg::new("dry-run")
                .help("Don't make any actual changes")
                .short('n')
                .long("dry-run")
                .action(ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("force")
                .help("Skip safety checks")
                .short('f')
                .long("force")
                .action(ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("verbose")
                .help("Display more output")
                .short('v')
                .long("verbose")
                .action(ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("and-rebase")
                .help("Run rebase if successful")
                .short('r')
                .long("and-rebase")
                .action(ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("gen-completions")
                .help("Generate completions")
                .long("gen-completions")
                .value_parser(["bash", "fish", "zsh", "powershell", "elvish"]),
        )
        .arg(
            clap::Arg::new("whole-file")
                .help("Match the change against the complete file   ")
                .short('w')
                .long("whole-file")
                .action(ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("one-fixup-per-commit")
                .help("Only generate one fixup per commit")
                .short('F')
                .long("one-fixup-per-commit")
                .action(ArgAction::SetTrue),
        );
    let mut args_clone = args.clone();
    let args = args.get_matches();

    if let Some(shell) = args.get_one::<String>("gen-completions") {
        let app_name = "git-absorb";
        match shell.as_str() {
            "bash" => {
                generate(Shell::Bash, &mut args_clone, app_name, &mut io::stdout());
            }
            "fish" => {
                generate(Shell::Fish, &mut args_clone, app_name, &mut io::stdout());
            }
            "zsh" => {
                generate(Shell::Zsh, &mut args_clone, app_name, &mut io::stdout());
            }
            "powershell" => {
                generate(
                    Shell::PowerShell,
                    &mut args_clone,
                    app_name,
                    &mut io::stdout(),
                );
            }
            "elvish" => {
                generate(Shell::Elvish, &mut args_clone, app_name, &mut io::stdout());
            }
            _ => unreachable!(),
        }
        return;
    }

    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    let drain = slog::LevelFilter::new(
        drain,
        if args.get_flag("verbose") {
            slog::Level::Debug
        } else {
            slog::Level::Info
        },
    )
    .fuse();
    let mut logger = slog::Logger::root(drain, o!());
    if args.get_flag("verbose") {
        logger = logger.new(o!(
            "module" => slog::FnValue(|record| record.module()),
            "line" => slog::FnValue(|record| record.line()),
        ));
    }

    if let Err(e) = git_absorb::run(&mut git_absorb::Config {
        dry_run: args.get_flag("dry-run"),
        force: args.get_flag("force"),
        base: args.get_one::<String>("base").map(|s| s.as_str()),
        and_rebase: args.get_flag("and-rebase"),
        whole_file: args.get_flag("whole-file"),
        one_fixup_per_commit: args.get_flag("one-fixup-per-commit"),
        logger: &logger,
    }) {
        crit!(logger, "absorb failed"; "err" => e.to_string());
        // wait for async logger to finish writing messages
        drop(logger);
        ::std::process::exit(1);
    }
}
