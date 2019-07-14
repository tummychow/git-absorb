extern crate failure;
extern crate git2;
extern crate slog;

use std::collections::HashMap;

pub const MAX_STACK_CONFIG_NAME: &str = "absorb.maxStack";
pub const MAX_STACK: usize = 10;

fn max_stack(repo: &git2::Repository) -> usize {
    match repo
        .config()
        .and_then(|config| config.get_i64(MAX_STACK_CONFIG_NAME))
    {
        Ok(max_stack) if max_stack > 0 => max_stack as usize,
        _ => MAX_STACK,
    }
}

pub fn working_stack<'repo>(
    repo: &'repo git2::Repository,
    base: Option<&git2::Commit<'repo>>,
    logger: &slog::Logger,
) -> Result<Vec<git2::Commit<'repo>>, failure::Error> {
    let head = repo.head()?;
    debug!(logger, "head found"; "head" => head.name());

    if !head.is_branch() {
        return Err(failure::err_msg("HEAD is not a branch"));
    }

    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL);
    revwalk.push_head()?;
    debug!(logger, "head pushed"; "head" => head.name());

    if let Some(base) = base {
        revwalk.hide(base.id())?;
        debug!(logger, "commit hidden"; "commit" => base.id().to_string());
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
    let sig = repo.signature()?;
    for rev in revwalk {
        let commit = repo.find_commit(rev?)?;
        if commit.parents().len() > 1 {
            warn!(logger, "merge commit found"; "commit" => commit.id().to_string());
            break;
        }
        if commit.author().name_bytes() != sig.name_bytes()
            || commit.author().email_bytes() != sig.email_bytes()
        {
            warn!(logger, "foreign author found"; "commit" => commit.id().to_string());
            break;
        }
        if ret.len() == max_stack(repo) {
            warn!(logger, "stack limit reached"; "limit" => ret.len());
            break;
        }
        debug!(logger, "commit pushed onto stack"; "commit" => commit.id().to_string());
        ret.push(commit);
    }
    Ok(ret)
}

pub fn summary_counts<'repo, 'a, I>(commits: I) -> HashMap<String, u64>
where
    I: IntoIterator<Item = &'a git2::Commit<'repo>>,
    // TODO: we have to use a hashmap of owned strings because the
    // commit summary has the 'a lifetime (the commit outlives this
    // function, but the reference to the commit does not), it would
    // be nice if the commit summary had the 'repo lifetime instead
    'repo: 'a,
{
    let mut ret = HashMap::new();
    for commit in commits {
        let count = ret
            // TODO: unnecessary allocation if key already exists
            .entry(commit.summary().unwrap_or("").to_owned())
            .or_insert(0);
        *count += 1;
    }
    ret
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
        // TODO: use in-memory ODB instead (blocked on git2 support)
        let repo = git2::Repository::init(&dir).unwrap();

        let mut config = repo.config().unwrap();
        config.set_str("user.name", "nobody").unwrap();
        config.set_str("user.email", "nobody@example.com").unwrap();

        (dir, repo)
    }

    fn empty_commit<'repo>(
        repo: &'repo git2::Repository,
        update_ref: &str,
        message: &str,
        parents: &[&git2::Commit],
    ) -> git2::Commit<'repo> {
        let sig = repo.signature().unwrap();
        let tree = repo
            .find_tree(repo.treebuilder(None).unwrap().write().unwrap())
            .unwrap();

        repo.find_commit(
            repo.commit(Some(update_ref), &sig, &sig, message, &tree, parents)
                .unwrap(),
        )
        .unwrap()
    }

    fn empty_commit_chain<'repo>(
        repo: &'repo git2::Repository,
        update_ref: &str,
        initial_parents: &[&git2::Commit],
        length: usize,
    ) -> Vec<git2::Commit<'repo>> {
        let mut ret = Vec::with_capacity(length);

        for idx in 0..length {
            let next = if let Some(last) = ret.last() {
                // TODO: how to deduplicate the rest of this call if last doesn't live long enough?
                empty_commit(repo, update_ref, &idx.to_string(), &[last])
            } else {
                empty_commit(repo, update_ref, &idx.to_string(), initial_parents)
            };
            ret.push(next)
        }

        assert_eq!(ret.len(), length);
        ret
    }

    fn assert_stack_matches_chain(length: usize, stack: &[git2::Commit], chain: &[git2::Commit]) {
        assert_eq!(stack.len(), length);
        for (chain_commit, stack_commit) in chain.iter().rev().take(length).zip(stack) {
            assert_eq!(stack_commit.id(), chain_commit.id());
        }
    }

    #[test]
    fn test_stack_hides_other_branches() {
        let (_dir, repo) = init_repo();
        let commits = empty_commit_chain(&repo, "HEAD", &[], 2);
        repo.branch("hide", &commits[0], false).unwrap();

        assert_stack_matches_chain(
            1,
            &working_stack(&repo, None, &empty_slog()).unwrap(),
            &commits,
        );
    }

    #[test]
    fn test_stack_uses_custom_base() {
        let (_dir, repo) = init_repo();
        let commits = empty_commit_chain(&repo, "HEAD", &[], 3);
        repo.branch("hide", &commits[1], false).unwrap();

        assert_stack_matches_chain(
            2,
            &working_stack(&repo, Some(&commits[0]), &empty_slog()).unwrap(),
            &commits,
        );
    }

    #[test]
    fn test_stack_stops_at_configured_limit() {
        let (_dir, repo) = init_repo();
        let commits = empty_commit_chain(&repo, "HEAD", &[], MAX_STACK + 2);
        repo.config()
            .unwrap()
            .set_i64(MAX_STACK_CONFIG_NAME, (MAX_STACK + 1) as i64)
            .unwrap();

        assert_stack_matches_chain(
            MAX_STACK + 1,
            &working_stack(&repo, None, &empty_slog()).unwrap(),
            &commits,
        );
    }

    #[test]
    fn test_stack_stops_at_foreign_author() {
        let (_dir, repo) = init_repo();
        let old_commits = empty_commit_chain(&repo, "HEAD", &[], 3);
        repo.config()
            .unwrap()
            .set_str("user.name", "nobody2")
            .unwrap();
        let new_commits = empty_commit_chain(&repo, "HEAD", &[old_commits.last().unwrap()], 2);

        assert_stack_matches_chain(
            2,
            &working_stack(&repo, None, &empty_slog()).unwrap(),
            &new_commits,
        );
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
        let commits = empty_commit_chain(&repo, "HEAD", &[&merge], 2);

        assert_stack_matches_chain(
            2,
            &working_stack(&repo, None, &empty_slog()).unwrap(),
            &commits,
        );
    }
}
