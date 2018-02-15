extern crate git2;
#[macro_use]
extern crate slog;

use std::error::Error;

pub struct Config<'a> {
    pub dry_run: bool,
    pub force: bool,
    pub logger: &'a slog::Logger,
}

pub fn run(config: &Config) -> Result<(), Box<Error>> {
    let repo = git2::Repository::open_from_env()?;
    debug!(config.logger, "repository found"; "path" => repo.path().to_str());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run() {
        assert!(
            run(&Config {
                dry_run: false,
                force: false,
                logger: &slog::Logger::root(slog::Discard, o!()),
            }).is_ok()
        );
    }
}
