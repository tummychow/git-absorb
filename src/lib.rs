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

fn working_stack<'repo>(
    repo: &'repo git2::Repository,
    logger: &slog::Logger,
) -> Result<Vec<git2::Commit<'repo>>, Box<error::Error>> {
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
        let commit = repo.find_commit(rev?)?;
        debug!(logger, "commit walked"; "commit" => format!("{}", commit.id()));
        ret.push(commit);
    }
    Ok(ret)
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use super::*;

    #[test]
    fn test_run() {
        run(&Config {
            dry_run: false,
            force: false,
            logger: &slog::Logger::root(slog::Discard, o!()),
        }).unwrap();
    }

    #[test]
    fn test_stack() {
        let dir = tempdir::TempDir::new("git-absorb").unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = repo.signature().unwrap();
        let tree = {
            let mut index = repo.index().unwrap();
            let id = index.write_tree().unwrap();
            repo.find_tree(id).unwrap()
        };
        let head = repo.find_commit(
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap(),
        ).unwrap();
        repo.branch("new", &head, false).unwrap();
        let next = repo.find_commit(
            repo.commit(Some("HEAD"), &sig, &sig, "next", &tree, &[&head])
                .unwrap(),
        ).unwrap();

        let stack = working_stack(&repo, &slog::Logger::root(slog::Discard, o!())).unwrap();
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].id(), next.id());
    }
}
