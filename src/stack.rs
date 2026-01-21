use anyhow::{anyhow, Result};

use std::collections::HashMap;

use crate::config;

#[derive(Debug, PartialEq)]
pub enum StackEndReason {
    ReachedRoot,
    ReachedMergeCommit,
    ReachedAnotherAuthor,
    ReachedLimit,
    CommitsHiddenByBase,
    CommitsHiddenByBranches,
}

pub fn working_stack<'repo>(
    repo: &'repo git2::Repository,
    no_limit: bool,
    user_provided_base: Option<&str>,
    force_author: bool,
    force_detach: bool,
    logger: &slog::Logger,
) -> Result<(Vec<git2::Commit<'repo>>, StackEndReason)> {
    let head = repo.head()?;
    debug!(logger, "head found"; "head" => head.name());

    if !head.is_branch() {
        if !force_detach {
            return Err(anyhow!(
                "HEAD is not a branch, use --force-detach to override"
            ));
        } else {
            warn!(
                logger,
                "HEAD is not a branch, but --force-detach used to continue."
            );
        }
    }

    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL)?;
    revwalk.push_head()?;
    revwalk.simplify_first_parent()?;
    debug!(logger, "head pushed"; "head" => head.name());

    let base_commit = match user_provided_base {
        // https://github.com/rust-lang/rfcs/issues/1815
        // user_provided_base isn't guaranteed to be a commit hash, so peel until a
        // commit is found.
        Some(commitish) => Some(repo.revparse_single(commitish)?.peel_to_commit()?),
        None => None,
    };

    if let Some(base_commit) = &base_commit {
        revwalk.hide(base_commit.id())?;
        debug!(logger, "commit hidden"; "commit" => base_commit.id().to_string());
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
    let mut stack_end_reason: Option<StackEndReason> = None;
    let sig = repo.signature();
    for rev in revwalk {
        let commit = repo.find_commit(rev?)?;
        if commit.parent_count() > 1 {
            debug!(logger, "Stack ends at merge commit"; "commit" => commit.id().to_string());
            return Ok((ret, StackEndReason::ReachedMergeCommit));
        }

        if !force_author && is_by_another_author(&sig, &commit) {
            debug!(logger, "Stopping before commit by another author.";
                  "commit" => commit.id().to_string());
            stack_end_reason = Some(StackEndReason::ReachedAnotherAuthor);
            break;
        }

        if !no_limit && ret.len() == config::max_stack(repo) && user_provided_base.is_none() {
            debug!(logger, "Stopping at stack limit.";
                  "limit" => ret.len());
            stack_end_reason = Some(StackEndReason::ReachedLimit);
            break;
        }

        debug!(logger, "commit pushed onto stack"; "commit" => commit.id().to_string());
        ret.push(commit);
    }

    match stack_end_reason {
        Some(end_reason) => Ok((ret, end_reason)),
        None => {
            // We walked off the available commits. Either we reached the root of the repository
            // or all the remaining commits are hidden.
            // Even if the next commit is hidden, it may have been rejected for other reasons,
            // such as being a merge commit. Find the most dire reason we couldn't use the next
            // commit (if any) and report it.
            let last_stack_commit = ret.last();
            let hidden_commit = match last_stack_commit {
                None => head.peel_to_commit()?,
                Some(commit) => {
                    if commit.parent_count() == 0 {
                        return Ok((ret, StackEndReason::ReachedRoot));
                    }
                    commit.parent(0)?
                }
            };

            if hidden_commit.parent_count() > 1 {
                return Ok((ret, StackEndReason::ReachedMergeCommit));
            }

            if !force_author && is_by_another_author(&sig, &hidden_commit) {
                return Ok((ret, StackEndReason::ReachedAnotherAuthor));
            }

            if user_provided_base.is_some() {
                Ok((ret, StackEndReason::CommitsHiddenByBase))
            } else {
                Ok((ret, StackEndReason::CommitsHiddenByBranches))
            }
        }
    }
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

fn is_by_another_author(
    sig: &Result<git2::Signature, git2::Error>,
    hidden_commit: &git2::Commit,
) -> bool {
    if let Ok(ref sig) = sig {
        hidden_commit.author().name_bytes() != sig.name_bytes()
            || hidden_commit.author().email_bytes() != sig.email_bytes()
    } else {
        false
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::tests::repo_utils;

    fn empty_slog() -> slog::Logger {
        slog::Logger::root(slog::Discard, o!())
    }

    fn init_repo() -> (tempfile::TempDir, git2::Repository) {
        // the repo will be deleted when the tempdir gets dropped
        let dir = tempfile::TempDir::new().unwrap();
        // TODO: use in-memory ODB instead (blocked on git2 support)
        let repo = git2::Repository::init(&dir).unwrap();

        let mut config = repo.config().unwrap();
        config.set_str("user.name", "nobody").unwrap();
        config.set_str("user.email", "nobody@example.com").unwrap();

        (dir, repo)
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
        let commits = repo_utils::empty_commit_chain(&repo, "HEAD", &[], 2);
        repo.branch("hide", &commits[0], false).unwrap();

        let (stack, reason) =
            working_stack(&repo, false, None, false, false, &empty_slog()).unwrap();
        assert_stack_matches_chain(1, &stack, &commits);
        assert_eq!(reason, StackEndReason::CommitsHiddenByBranches);
    }

    #[test]
    fn test_stack_uses_custom_base() {
        let (_dir, repo) = init_repo();
        let commits = repo_utils::empty_commit_chain(&repo, "HEAD", &[], 3);
        repo.branch("hide", &commits[1], false).unwrap();

        let (stack, reason) = working_stack(
            &repo,
            false,
            Some(&commits[0].id().to_string()),
            false,
            false,
            &empty_slog(),
        )
        .unwrap();
        assert_stack_matches_chain(2, &stack, &commits);
        assert_eq!(reason, StackEndReason::CommitsHiddenByBase);
    }

    #[test]
    fn test_stack_stops_at_configured_limit() {
        let (_dir, repo) = init_repo();
        let commits = repo_utils::empty_commit_chain(&repo, "HEAD", &[], config::MAX_STACK + 2);
        repo.config()
            .unwrap()
            .set_i64(
                config::MAX_STACK_CONFIG_NAME,
                (config::MAX_STACK + 1) as i64,
            )
            .unwrap();

        let (stack, reason) =
            working_stack(&repo, false, None, false, false, &empty_slog()).unwrap();
        assert_stack_matches_chain(config::MAX_STACK + 1, &stack, &commits);
        assert_eq!(reason, StackEndReason::ReachedLimit);
    }

    #[test]
    fn test_stack_stops_at_another_author() {
        let (_dir, repo) = init_repo();
        let old_commits = repo_utils::empty_commit_chain(&repo, "HEAD", &[], 3);
        repo.config()
            .unwrap()
            .set_str("user.name", "nobody2")
            .unwrap();
        let new_commits =
            repo_utils::empty_commit_chain(&repo, "HEAD", &[old_commits.last().unwrap()], 2);

        let (stack, reason) =
            working_stack(&repo, false, None, false, false, &empty_slog()).unwrap();
        assert_stack_matches_chain(2, &stack, &new_commits);
        assert_eq!(reason, StackEndReason::ReachedAnotherAuthor);
    }

    #[test]
    fn test_stack_stops_at_merges() {
        let (_dir, repo) = init_repo();
        let merge = repo_utils::merge_commit(&repo, &[]);
        let commits = repo_utils::empty_commit_chain(&repo, "HEAD", &[&merge], 2);

        let (stack, reason) =
            working_stack(&repo, false, None, false, false, &empty_slog()).unwrap();
        assert_stack_matches_chain(2, &stack, &commits);
        assert_eq!(reason, StackEndReason::ReachedMergeCommit);
    }
}
