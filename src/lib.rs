extern crate git2;
#[macro_use]
extern crate slog;

use std::error;
use std::fmt;

pub struct Config<'a> {
    pub dry_run: bool,
    pub force: bool,
    pub base: Option<&'a str>,
    pub logger: &'a slog::Logger,
}

pub fn run(config: &Config) -> Result<(), Box<error::Error>> {
    let repo = git2::Repository::open_from_env()?;
    debug!(config.logger, "repository found"; "path" => repo.path().to_str());

    let base = match config.base {
        // https://github.com/rust-lang/rfcs/issues/1815
        Some(commitish) => Some(repo.find_commit(repo.revparse_single(commitish)?.id())?),
        None => None,
    };

    working_stack(&repo, base, config.logger)?;

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
    custom_base: Option<git2::Commit<'repo>>,
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

    if let Some(base_commit) = custom_base {
        revwalk.hide(base_commit.id())?;
        debug!(logger, "commit hidden"; "commit" => format!("{}", base_commit.id()));
    } else {
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
    }

    let mut ret = Vec::new();
    for rev in revwalk {
        let commit = repo.find_commit(rev?)?;
        if commit.parents().count() > 1 {
            debug!(logger, "merge commit found"; "commit" => format!("{}", commit.id()));
            break;
        }
        debug!(logger, "commit pushed onto stack"; "commit" => format!("{}", commit.id()));
        ret.push(commit);
    }
    Ok(ret)
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use super::*;

    fn empty_slog() -> slog::Logger {
        slog::Logger::root(slog::Discard, o!())
    }

    fn init_repo() -> (tempdir::TempDir, git2::Repository) {
        // the repo will be deleted when the tempdir gets dropped
        let dir = tempdir::TempDir::new("git-absorb").unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        (dir, repo)
    }

    fn empty_commit<'repo>(
        repo: &'repo git2::Repository,
        update_ref: &str,
        message: &str,
        parents: &[&git2::Commit],
    ) -> git2::Commit<'repo> {
        let sig = git2::Signature::now("nobody", "nobody@example.com").unwrap();
        let tree = repo.find_tree(repo.treebuilder(None).unwrap().write().unwrap())
            .unwrap();

        repo.find_commit(
            repo.commit(Some(update_ref), &sig, &sig, message, &tree, parents)
                .unwrap(),
        ).unwrap()
    }

    #[test]
    fn test_stack_hides_other_branches() {
        let (_dir, repo) = init_repo();
        let first = empty_commit(&repo, "HEAD", "first", &[]);
        let second = empty_commit(&repo, "HEAD", "second", &[&first]);
        repo.branch("hide", &first, false).unwrap();

        let stack = working_stack(&repo, None, &empty_slog()).unwrap();
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].id(), second.id());
    }

    #[test]
    fn test_stack_uses_custom_base() {
        let (_dir, repo) = init_repo();
        let first = empty_commit(&repo, "HEAD", "first", &[]);
        let second = empty_commit(&repo, "HEAD", "second", &[&first]);
        let third = empty_commit(&repo, "HEAD", "third", &[&second]);
        repo.branch("hide", &second, false).unwrap();

        let stack = working_stack(&repo, Some(first), &empty_slog()).unwrap();
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[0].id(), third.id());
        assert_eq!(stack[1].id(), second.id());
    }

    #[test]
    fn test_stack_stops_at_merges() {
        let (_dir, repo) = init_repo();
        let first = empty_commit(&repo, "HEAD", "first", &[]);
        // equivalent to checkout --orphan
        repo.set_head("refs/heads/new").unwrap();
        let second = empty_commit(&repo, "HEAD", "second", &[]);
        // the current commit must be the first parent
        let merge = empty_commit(&repo, "HEAD", "merge", &[&second, &first]);
        let last = empty_commit(&repo, "HEAD", "last", &[&merge]);

        let stack = working_stack(&repo, None, &empty_slog()).unwrap();
        assert_eq!(stack.len(), 1);
        assert_eq!(stack[0].id(), last.id());
    }
}
