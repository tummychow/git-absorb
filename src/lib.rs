extern crate git2;
#[macro_use]
extern crate slog;

use std::error;
use std::fmt;

pub struct Config<'a> {
    pub dry_run: bool,
    pub force: bool,
    pub logger: &'a slog::Logger,
}

pub fn run(config: &Config) -> Result<(), Box<error::Error>> {
    let repo = git2::Repository::open_from_env()?;
    debug!(config.logger, "repository found"; "path" => repo.path().to_str());

    working_stack(&repo, config.logger)?;

    Ok(())
}

#[derive(Debug)]
pub struct Error(String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}
impl error::Error for Error {
    fn description(&self) -> &str {
        &self.0
    }
    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

fn working_stack(
    repo: &git2::Repository,
    logger: &slog::Logger,
) -> Result<Vec<git2::Oid>, Box<error::Error>> {
    let head = repo.head()?;
    debug!(logger, "head found"; "head" => head.name());

    if !head.is_branch() {
        return Err(Box::new(Error(String::from("HEAD is not a branch"))));
    }

    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::SORT_TOPOLOGICAL);
    revwalk.push_head()?;
    debug!(logger, "head pushed"; "head" => head.name());

    for branch in repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = branch?;
        let branch = branch.get().name();

        match branch {
            Some(name) if Some(name) != head.name() => {
                revwalk.hide_ref(name)?;
                debug!(logger, "branch hidden"; "branch" => branch);
            }
            _ => {
                debug!(logger, "branch not hidden"; "branch" => branch);
            }
        };
    }

    let mut ret = Vec::new();
    for rev in revwalk {
        let rev = rev?;
        ret.push(rev);
        debug!(logger, "rev walked"; "rev" => format!("{}", rev));
    }
    Ok(ret)
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
