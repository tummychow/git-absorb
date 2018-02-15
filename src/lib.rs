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

    let head = repo.head()?;
    debug!(config.logger, "head found"; "head" => head.name());

    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::SORT_TOPOLOGICAL);
    revwalk.push_head()?;

    for branch in repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = branch?;
        let branch = branch.get().name();

        match branch {
            Some(name) if Some(name) != head.name() => {
                revwalk.hide_ref(name)?;
                debug!(config.logger, "branch hidden"; "branch" => branch);
            }
            _ => {
                debug!(config.logger, "branch not hidden"; "branch" => branch);
            }
        };
    }

    for rev in revwalk {
        let rev = rev?;
        debug!(config.logger, "rev walked"; "rev" => format!("{}", rev));
    }

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
