extern crate failure;
extern crate git2;
#[macro_use]
extern crate slog;

pub const MAX_STACK_CONFIG_NAME: &str = "absorb.maxStack";
pub const MAX_STACK: usize = 10;

pub struct Config<'a> {
    pub dry_run: bool,
    pub force: bool,
    pub base: Option<&'a str>,
    pub logger: &'a slog::Logger,
}

pub fn run(config: &Config) -> Result<(), failure::Error> {
    let repo = git2::Repository::open_from_env()?;
    debug!(config.logger, "repository found"; "path" => repo.path().to_str());

    let base = match config.base {
        // https://github.com/rust-lang/rfcs/issues/1815
        Some(commitish) => Some(repo.find_commit(repo.revparse_single(commitish)?.id())?),
        None => None,
    };

    let stack = working_stack(&repo, base, config.logger)?;

    let index = repo.diff_tree_to_index(
        Some(&repo.head()?.peel_to_tree()?),
        None,
        Some(&mut diff_options()),
    )?;
    debug!(config.logger, "parsed index";
           "index" => format!("{:?}", parse_diff(&index)?),
    );

    Ok(())
}

fn parse_diff(diff: &git2::Diff) -> Result<Vec<OwnedPatch>, failure::Error> {
    let mut ret = Vec::new();
    for (delta_idx, _delta) in diff.deltas().enumerate() {
        ret.push(OwnedPatch::new(&mut git2::Patch::from_diff(
            diff,
            delta_idx,
        )?.ok_or(failure::err_msg(
            "got empty delta",
        ))?)?);
    }
    Ok((ret))
}

#[derive(Debug)]
struct OwnedBlock {
    start: u32,
    lines: Vec<Vec<u8>>,
    trailing_newline: bool,
}
#[derive(Debug)]
struct OwnedHunk {
    added: OwnedBlock,
    removed: OwnedBlock,
}
impl OwnedHunk {
    fn new(patch: &mut git2::Patch, idx: usize) -> Result<OwnedHunk, failure::Error> {
        let mut ret = {
            let (hunk, _size) = patch.hunk(idx)?;
            OwnedHunk {
                added: OwnedBlock {
                    start: hunk.new_start(),
                    lines: Vec::with_capacity(hunk.new_lines() as usize),
                    trailing_newline: true,
                },
                removed: OwnedBlock {
                    start: hunk.old_start(),
                    lines: Vec::with_capacity(hunk.old_lines() as usize),
                    trailing_newline: true,
                },
            }
        };

        for line_idx in 0..patch.num_lines_in_hunk(idx)? {
            let line = patch.line_in_hunk(idx, line_idx)?;
            match line.origin() {
                '+' => {
                    if line.num_lines() > 1 {
                        return Err(failure::err_msg("wrong number of lines in hunk"));
                    }
                    if line.new_lineno()
                        .ok_or(failure::err_msg("added line did not have lineno"))?
                        != ret.added.start + ret.added.lines.len() as u32
                    {
                        return Err(failure::err_msg("added line did not reach expected lineno"));
                    }
                    ret.added.lines.push(Vec::from(line.content()))
                }
                '-' => {
                    if line.num_lines() > 1 {
                        return Err(failure::err_msg("wrong number of lines in hunk"));
                    }
                    if line.old_lineno()
                        .ok_or(failure::err_msg("removed line did not have lineno"))?
                        != ret.removed.start + ret.removed.lines.len() as u32
                    {
                        return Err(failure::err_msg(
                            "removed line did not reach expected lineno",
                        ));
                    }
                    ret.removed.lines.push(Vec::from(line.content()))
                }
                '>' => {
                    if !ret.removed.trailing_newline {
                        return Err(failure::err_msg("removed nneof was already detected"));
                    };
                    ret.removed.trailing_newline = false
                }
                '<' => {
                    if !ret.added.trailing_newline {
                        return Err(failure::err_msg("added nneof was already detected"));
                    };
                    ret.added.trailing_newline = false
                }
                _ => {
                    return Err(failure::err_msg(format!(
                        "unknown line type {:?}",
                        line.origin()
                    )))
                }
            };
        }

        {
            let (hunk, _size) = patch.hunk(idx)?;
            if ret.added.lines.len() != hunk.new_lines() as usize {
                return Err(failure::err_msg("hunk added block size mismatch"));
            }
            if ret.removed.lines.len() != hunk.old_lines() as usize {
                return Err(failure::err_msg("hunk removed block size mismatch"));
            }
        }

        Ok(ret)
    }
}
#[derive(Debug)]
struct OwnedPatch {
    old_path: Option<Vec<u8>>,
    old_id: git2::Oid,
    new_path: Option<Vec<u8>>,
    new_id: git2::Oid,
    status: git2::Delta,
    hunks: Vec<OwnedHunk>,
}
impl OwnedPatch {
    fn new(patch: &mut git2::Patch) -> Result<OwnedPatch, failure::Error> {
        let mut ret = OwnedPatch {
            old_path: patch.delta().old_file().path_bytes().map(Vec::from),
            old_id: patch.delta().old_file().id(),
            new_path: patch.delta().new_file().path_bytes().map(Vec::from),
            new_id: patch.delta().new_file().id(),
            status: patch.delta().status(),
            hunks: Vec::with_capacity(patch.num_hunks()),
        };
        if patch.delta().nfiles() < 1 || patch.delta().nfiles() > 2 {
            return Err(failure::err_msg("delta with multiple files"));
        }

        for idx in 0..patch.num_hunks() {
            ret.hunks.push(OwnedHunk::new(patch, idx)?);
        }

        Ok(ret)
    }
}

fn diff_options() -> git2::DiffOptions {
    let mut ret = git2::DiffOptions::new();
    ret.context_lines(0)
        .id_abbrev(40)
        .ignore_filemode(true)
        .ignore_submodules(true);
    ret
}

fn max_stack(repo: &git2::Repository) -> usize {
    match repo.config()
        .and_then(|config| config.get_i64(MAX_STACK_CONFIG_NAME))
    {
        Ok(max_stack) if max_stack > 0 => max_stack as usize,
        _ => MAX_STACK,
    }
}

fn working_stack<'repo>(
    repo: &'repo git2::Repository,
    custom_base: Option<git2::Commit<'repo>>,
    logger: &slog::Logger,
) -> Result<Vec<git2::Commit<'repo>>, failure::Error> {
    let head = repo.head()?;
    debug!(logger, "head found"; "head" => head.name());

    if !head.is_branch() {
        return Err(failure::err_msg("HEAD is not a branch"));
    }

    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(git2::SORT_TOPOLOGICAL);
    revwalk.push_head()?;
    debug!(logger, "head pushed"; "head" => head.name());

    if let Some(base_commit) = custom_base {
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
    let sig = repo.signature()?;
    for rev in revwalk {
        let commit = repo.find_commit(rev?)?;
        if commit.parents().count() > 1 {
            debug!(logger, "merge commit found"; "commit" => commit.id().to_string());
            break;
        }
        if commit.author().name_bytes() != sig.name_bytes()
            || commit.author().email_bytes() != sig.email_bytes()
        {
            debug!(logger, "foreign author found"; "commit" => commit.id().to_string());
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
        let tree = repo.find_tree(repo.treebuilder(None).unwrap().write().unwrap())
            .unwrap();

        repo.find_commit(
            repo.commit(Some(update_ref), &sig, &sig, message, &tree, parents)
                .unwrap(),
        ).unwrap()
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

        // TODO: working_stack should take Option<&Commit>, to remove this clone()
        assert_stack_matches_chain(
            2,
            &working_stack(&repo, Some(commits[0].clone()), &empty_slog()).unwrap(),
            &commits,
        );
    }

    #[test]
    fn test_stack_stops_at_default_limit() {
        let (_dir, repo) = init_repo();
        let commits = empty_commit_chain(&repo, "HEAD", &[], MAX_STACK + 1);

        assert_stack_matches_chain(
            MAX_STACK,
            &working_stack(&repo, None, &empty_slog()).unwrap(),
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
