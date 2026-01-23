#[macro_use]
extern crate slog;

use clap::{CommandFactory, Parser as _};
use clap_complete::{generate, Shell};
use clap_complete_nushell::Nushell;
use slog::{Drain, Record};
use slog_term::{CountingWriter, RecordDecorator, ThreadSafeTimestampFn};
use std::io;
use std::io::Write;

/// Automatically absorb staged changes into your current branch
#[derive(Debug, clap::Parser)]
#[command(version)]
struct Cli {
    /// Use this commit as the base of the absorb stack
    #[clap(long, short)]
    base: Option<String>,
    /// Don't make any actual changes
    #[clap(long, short = 'n')]
    dry_run: bool,
    /// Remove absorb stack limit. Be careful with this
    #[clap(long)]
    no_limit: bool,
    /// Generate fixups to commits not made by you
    #[clap(long)]
    force_author: bool,
    /// Generate fixups even when on a non-branch (detached) HEAD
    #[clap(long)]
    force_detach: bool,
    /// Skip all safety checks as if all --force-* flags were given
    #[clap(long, short)]
    force: bool,
    /// Display more output
    #[clap(long, short)]
    verbose: bool,
    /// Run rebase if successful
    #[clap(long, short = 'r')]
    and_rebase: bool,
    /// Extra arguments to pass to git rebase. Only valid if --and-rebase is set
    #[clap(last = true)]
    rebase_options: Vec<String>,
    /// Generate completions
    #[clap(long, value_name = "SHELL", value_parser = ["bash", "fish", "nushell", "zsh", "powershell", "elvish"])]
    gen_completions: Option<String>,
    /// Match the change against the complete file
    #[clap(long, short)]
    whole_file: bool,
    /// Only generate one fixup per commit
    #[clap(long, short = 'F')]
    one_fixup_per_commit: bool,
    /// Create squash commits instead of fixup
    #[clap(long, short = 's')]
    squash: bool,
    /// Commit message body that is given to all fixup commits
    #[clap(long, short)]
    message: Option<String>,
}

fn main() {
    let Cli {
        base,
        dry_run,
        no_limit,
        force_author,
        force_detach,
        force,
        verbose,
        and_rebase,
        rebase_options,
        gen_completions,
        whole_file,
        one_fixup_per_commit,
        squash,
        message,
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
    let drain = slog_term::FullFormat::new(decorator)
        .use_custom_header_print(print_msg_header)
        .build()
        .fuse();
    let drain = std::sync::Mutex::new(drain).fuse();

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

    let rebase_options: Vec<&str> = rebase_options.iter().map(AsRef::as_ref).collect();
    if let Err(e) = git_absorb::run(
        &logger,
        &git_absorb::Config {
            dry_run,
            force_author: force_author || force,
            force_detach: force_detach || force,
            base: base.as_deref(),
            and_rebase,
            rebase_options: &rebase_options,
            no_limit,
            whole_file,
            one_fixup_per_commit,
            squash,
            message: message.as_deref(),
        },
    ) {
        crit!(logger, "absorb failed"; "err" => e.to_string());
        // wait for async logger to finish writing messages
        drop(logger);
        ::std::process::exit(1);
    }
}

pub fn print_msg_header(
    _fn_timestamp: &dyn ThreadSafeTimestampFn<Output = io::Result<()>>,
    mut rd: &mut dyn RecordDecorator,
    record: &Record,
    _use_file_location: bool,
) -> io::Result<bool> {
    rd.start_level()?; // color-codes the message
    let mut count_rd = CountingWriter::new(&mut rd);
    write!(count_rd, "{}", record.msg())?;
    Ok(count_rd.count() != 0)
}
