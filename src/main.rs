#[macro_use]
extern crate slog;

use clap::{CommandFactory, Parser as _};
use clap_complete::{generate, Shell};
use clap_complete_nushell::Nushell;
use slog::Drain;
use std::io;

/// Automatically absorb staged changes into your current branch
#[derive(Debug, clap::Parser)]
struct Cli {
    /// Use this commit as the base of the absorb stack
    #[clap(long, short)]
    base: Option<String>,
    /// Don't make any actual changes
    #[clap(long, short = 'n')]
    dry_run: bool,
    /// Skip safety checks
    #[clap(long, short)]
    force: bool,
    /// Display more output
    #[clap(long, short)]
    verbose: bool,
    /// Run rebase if successful
    #[clap(long, short = 'r')]
    and_rebase: bool,
    /// Generate completions
    #[clap(long, value_parser = ["bash", "fish", "nushell", "zsh", "powershell", "elvish"])]
    gen_completions: Option<String>,
    /// Match the change against the complete file
    #[clap(long, short)]
    whole_file: bool,
    /// Only generate one fixup per commit
    #[clap(long, short = 'F')]
    one_fixup_per_commit: bool,
}

fn main() {
    let Cli {
        base,
        dry_run,
        force,
        verbose,
        and_rebase,
        gen_completions,
        whole_file,
        one_fixup_per_commit,
    } = Cli::parse();

    if let Some(shell) = gen_completions {
        let app_name = "git-absorb";
        let mut cmd = Cli::command();
        match shell.as_str() {
            "bash" => generate(Shell::Bash, &mut cmd, app_name, &mut io::stdout()),
            "fish" => generate(Shell::Fish, &mut cmd, app_name, &mut io::stdout()),
            "nushell" => generate(Nushell, &mut cmd, app_name, &mut io::stdout()),
            "zsh" => generate(Shell::Zsh, &mut cmd, app_name, &mut io::stdout()),
            "powershell" => generate(Shell::PowerShell, &mut cmd, app_name, &mut io::stdout()),
            "elvish" => generate(Shell::Elvish, &mut cmd, app_name, &mut io::stdout()),
            _ => unreachable!(),
        }
        return;
    }

    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    let drain = slog::LevelFilter::new(
        drain,
        if verbose {
            slog::Level::Debug
        } else {
            slog::Level::Info
        },
    )
    .fuse();
    let mut logger = slog::Logger::root(drain, o!());
    if verbose {
        logger = logger.new(o!(
            "module" => slog::FnValue(|record| record.module()),
            "line" => slog::FnValue(|record| record.line()),
        ));
    }

    if let Err(e) = git_absorb::run(&mut git_absorb::Config {
        dry_run,
        force,
        base: base.as_deref(),
        and_rebase,
        whole_file,
        one_fixup_per_commit,
        logger: &logger,
    }) {
        crit!(logger, "absorb failed"; "err" => e.to_string());
        // wait for async logger to finish writing messages
        drop(logger);
        ::std::process::exit(1);
    }
}
