#[macro_use]
extern crate slog;
use anyhow::{anyhow, Result};

mod commute;
mod config;
mod owned;
mod stack;

use std::io::Write;
use std::path::Path;

type WorkingStack<'a> = Vec<(git2::Commit<'a>, owned::Diff)>;

pub struct Config<'a> {
    pub dry_run: bool,
    pub force_author: bool,
    pub force_detach: bool,
    pub base: Option<&'a str>,
    pub and_rebase: bool,
    pub rebase_options: &'a Vec<&'a str>,
    pub whole_file: bool,
    pub one_fixup_per_commit: bool,
}

pub fn run(logger: &slog::Logger, config: &Config) -> Result<()> {
    let repo = git2::Repository::open_from_env()?;
    debug!(logger, "repository found"; "path" => repo.path().to_str());

    run_with_repo(&logger, &config, &repo)
}

fn run_with_repo(logger: &slog::Logger, config: &Config, repo: &git2::Repository) -> Result<()> {
    if !config.rebase_options.is_empty() && !config.and_rebase {
        return Err(anyhow!(
            "REBASE_OPTIONS were specified without --and-rebase flag"
        ));
    }

    let config = config::unify(&config, repo);
    let stack = stack::working_stack(
        repo,
        config.base,
        config.force_author,
        config.force_detach,
        logger,
    )?;
    if stack.is_empty() {
        crit!(logger, "No commits available to fix up, exiting");
        return Ok(());
    }

    let autostage_enabled = config::auto_stage_if_nothing_staged(repo);
    let index_was_empty = nothing_left_in_index(repo)?;
    let mut we_added_everything_to_index = false;
    if autostage_enabled && index_was_empty {
        we_added_everything_to_index = true;

        // no matter from what subdirectory we're executing,
        // "." will still refer to the root workdir.
        let pathspec = ["."];
        let mut index = repo.index()?;
        index.add_all(pathspec.iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;
    }

    let mut diff_options = Some({
        let mut ret = git2::DiffOptions::new();
        ret.context_lines(0)
            .id_abbrev(40)
            .ignore_filemode(true)
            .ignore_submodules(true);
        ret
    });

    let (stack, summary_counts): (Vec<_>, _) = {
        let mut diffs = Vec::with_capacity(stack.len());
        for commit in &stack {
            let diff = owned::Diff::new(
                &repo.diff_tree_to_tree(
                    if commit.parents().len() == 0 {
                        None
                    } else {
                        Some(commit.parent(0)?.tree()?)
                    }
                    .as_ref(),
                    Some(&commit.tree()?),
                    diff_options.as_mut(),
                )?,
            )?;
            trace!(logger, "parsed commit diff";
                   "commit" => commit_id(commit),
                   "diff" => format!("{:?}", diff),
            );
            diffs.push(diff);
        }

        let summary_counts = stack::summary_counts(&stack);
        (stack.into_iter().zip(diffs).collect(), summary_counts)
    };

    let rebase_commit = get_rebase_commit_from_stack(&stack)?;
    info!(logger, "base commit selected"; "commit" => commit_id(&rebase_commit), "message" => rebase_commit.summary());

    let mut head_tree = repo.head()?.peel_to_tree()?;
    let index = owned::Diff::new(&repo.diff_tree_to_index(
        Some(&head_tree),
        None,
        diff_options.as_mut(),
    )?)?;
    trace!(logger, "parsed index";
           "index" => format!("{:?}", index),
    );

    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("nobody", "nobody@example.com"))?;
    let mut head_commit = repo.head()?.peel_to_commit()?;

    let mut hunks_with_commit = vec![];

    let mut patches_considered = 0usize;
    'patch: for index_patch in index.iter() {
        let old_path = index_patch.new_path.as_slice();
        if index_patch.status != git2::Delta::Modified {
            debug!(logger, "skipped non-modified hunk";
                    "path" => String::from_utf8_lossy(old_path).into_owned(),
                    "status" => format!("{:?}", index_patch.status),
            );
            continue 'patch;
        }

        patches_considered += 1;

        let mut preceding_hunks_offset = 0isize;
        let mut applied_hunks_offset = 0isize;
        'hunk: for index_hunk in &index_patch.hunks {
            debug!(logger, "next hunk";
                   "header" => index_hunk.header(),
                   "path" => String::from_utf8_lossy(old_path).into_owned(),
            );

            // To properly handle files ("patches" in libgit2 lingo) with multiple hunks, we
            // need to find the updated line coordinates (`header`) of the current hunk in
            // two cases:
            // 1) As if it were the only hunk in the index. This only involves shifting the
            // "added" side *up* by the offset introduced by the preceding hunks:
            let isolated_hunk = index_hunk
                .clone()
                .shift_added_block(-preceding_hunks_offset);

            // 2) When applied on top of the previously committed hunks. This requires shifting
            // both the "added" and the "removed" sides of the previously isolated hunk *down*
            // by the offset of the committed hunks:
            let hunk_to_apply = isolated_hunk
                .clone()
                .shift_both_blocks(applied_hunks_offset);

            // The offset is the number of lines added minus the number of lines removed by a hunk:
            let hunk_offset = index_hunk.changed_offset();

            // To aid in understanding these arithmetic, here's an illustration.
            // There are two hunks in the original patch, each adding one line ("line2" and
            // "line5"). Assuming the first hunk (with offset = -1) was already processed
            // and applied, the table shows the three versions of the patch, with line numbers
            // on the <A>dded and <R>emoved sides for each:
            // |----------------|-----------|------------------|
            // |                |           | applied on top   |
            // | original patch | isolated  | of the preceding |
            // |----------------|-----------|------------------|
            // | <R> <A>        | <R> <A>   | <R> <A>          |
            // |----------------|-----------|------------------|
            // |  1   1  line1  |  1   1    |  1   1   line1   |
            // |  2      line2  |  2   2    |  2   2   line3   |
            // |  3   2  line3  |  3   3    |  3   3   line4   |
            // |  4   3  line4  |  4   4    |  4       line5   |
            // |  5      line5  |  5        |                  |
            // |----------------|-----------|------------------|
            // |       So the second hunk's `header` is:       |
            // |   -5,1 +3,0    | -5,1 +4,0 |    -4,1 +3,0     |
            // |----------------|-----------|------------------|

            debug!(logger, "";
                "to apply" => hunk_to_apply.header(),
                "to commute" => isolated_hunk.header(),
                "preceding hunks" => format!("{}/{}", applied_hunks_offset, preceding_hunks_offset),
            );

            preceding_hunks_offset += hunk_offset;

            // find the newest commit that the hunk cannot commute with
            let mut dest_commit = None;
            let mut commuted_old_path = old_path;
            let mut commuted_index_hunk = isolated_hunk;

            'commit: for (commit, diff) in &stack {
                let c_logger = logger.new(o!(
                    "commit" => commit_id(commit),
                ));
                let next_patch = match diff.by_new(commuted_old_path) {
                    Some(patch) => patch,
                    // this commit doesn't touch the hunk's file, so
                    // they trivially commute, and the next commit
                    // should be considered
                    None => {
                        debug!(c_logger, "skipped commit with no path");
                        continue 'commit;
                    }
                };

                // sometimes we just forget some change (eg: intializing some object) that
                // happens in a completely unrelated place with the current hunks. In those
                // cases, might be helpful to just match the first commit touching the same
                // file as the current hunk. Use this option with care!
                if config.whole_file {
                    debug!(
                        c_logger,
                        "Commit touches the hunk file and match whole file is enabled"
                    );
                    dest_commit = Some(commit);
                    break 'commit;
                }

                if next_patch.status == git2::Delta::Added {
                    debug!(c_logger, "found noncommutative commit by add");
                    dest_commit = Some(commit);
                    break 'commit;
                }
                if commuted_old_path != next_patch.old_path.as_slice() {
                    debug!(c_logger, "changed commute path";
                           "path" => String::from_utf8_lossy(&next_patch.old_path).into_owned(),
                    );
                    commuted_old_path = next_patch.old_path.as_slice();
                }
                commuted_index_hunk = match commute::commute_diff_before(
                    &commuted_index_hunk,
                    &next_patch.hunks,
                ) {
                    Some(hunk) => {
                        debug!(c_logger, "commuted hunk with commit";
                               "offset" => (hunk.added.start as i64) - (commuted_index_hunk.added.start as i64),
                        );
                        hunk
                    }
                    // this commit contains a hunk that cannot
                    // commute with the hunk being absorbed
                    None => {
                        debug!(c_logger, "found noncommutative commit by conflict");
                        dest_commit = Some(commit);
                        break 'commit;
                    }
                };
            }
            let dest_commit = match dest_commit {
                Some(commit) => commit,
                // the hunk commutes with every commit in the stack,
                // so there is no commit to absorb it into
                None => {
                    warn!(
                        logger,
                        "Could not find a commit to fix up, use \
                         --base to increase the search range."
                    );
                    continue 'hunk;
                }
            };

            let hunk_with_commit = HunkWithCommit {
                hunk_to_apply,
                dest_commit,
                index_patch,
            };
            hunks_with_commit.push(hunk_with_commit);

            applied_hunks_offset += hunk_offset;
        }
    }

    let target_always_sha: bool = config::fixup_target_always_sha(repo);

    if !config.dry_run {
        repo.reference("PRE_ABSORB_HEAD", head_commit.id(), true, "")?;
    }

    // * apply all hunks that are going to be fixed up into `dest_commit`
    // * commit the fixup
    // * repeat for all `dest_commit`s
    //
    // the `.zip` here will gives us something similar to `.windows`, but with
    // an extra iteration for the last element (otherwise we would have to
    // special case the last element and commit it separately)
    for (current, next) in hunks_with_commit
        .iter()
        .zip(hunks_with_commit.iter().skip(1).map(Some).chain([None]))
    {
        let new_head_tree = apply_hunk_to_tree(
            repo,
            &head_tree,
            &current.hunk_to_apply,
            &current.index_patch.old_path,
        )?;

        // whether there are no more hunks to apply to `dest_commit`
        let commit_fixup = next.map_or(true, |next| {
            // if the next hunk is for a different commit -- commit what we have so far
            !config.one_fixup_per_commit || next.dest_commit.id() != current.dest_commit.id()
        });
        if commit_fixup {
            // TODO: the git2 api only supports utf8 commit messages,
            // so it's okay to use strings instead of bytes here
            // https://docs.rs/git2/0.7.5/src/git2/repo.rs.html#998
            // https://libgit2.org/libgit2/#HEAD/group/commit/git_commit_create
            let dest_commit_id = commit_id(current.dest_commit);
            let dest_commit_locator = match target_always_sha {
                true => &dest_commit_id,
                false => current
                    .dest_commit
                    .summary()
                    .filter(|&msg| summary_counts[msg] == 1)
                    .unwrap_or(&dest_commit_id),
            };
            let diff = repo
                .diff_tree_to_tree(Some(&head_commit.tree()?), Some(&new_head_tree), None)?
                .stats()?;
            if !config.dry_run {
                head_tree = new_head_tree;
                head_commit = repo.find_commit(repo.commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    &format!("fixup! {}\n", dest_commit_locator),
                    &head_tree,
                    &[&head_commit],
                )?)?;
                info!(logger, "committed";
                      "commit" => commit_id(&head_commit),
                      "header" => format!("+{},-{}", diff.insertions(), diff.deletions()),
                );
            } else {
                info!(logger, "would have committed";
                      "fixup" => dest_commit_locator,
                      "header" => format!("+{},-{}", diff.insertions(), diff.deletions()),
                );
            }
        } else {
            // we didn't commit anything, but we applied a hunk
            head_tree = new_head_tree;
        }
    }

    if autostage_enabled && we_added_everything_to_index {
        // now that the fixup commits have been created,
        // we should unstage the remaining changes from the index.

        let mut index = repo.index()?;
        index.read_tree(&head_tree)?;
        index.write()?;
    }

    if patches_considered == 0 {
        if index_was_empty && !we_added_everything_to_index {
            warn!(
                logger,
                "No changes staged, try adding something \
                 to the index or set {} = true",
                config::AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME
            );
        } else {
            warn!(
                logger,
                "Could not find a commit to fix up, use \
                 --base to increase the search range."
            )
        }
    } else if config.and_rebase {
        use std::process::Command;
        // unwrap() is safe here, as we exit early if the stack is empty
        let last_commit_in_stack = &stack.last().unwrap().0;
        // The stack isn't supposed to have any merge commits, per the check in working_stack()
        let number_of_parents = last_commit_in_stack.parents().len();
        assert!(number_of_parents <= 1);

        let mut command = Command::new("git");

        // We'd generally expect to be run from within the repository, but just in case,
        // try to have git run rebase from the repository root.
        // This simplifies writing tests that execute from within git-absorb's source directory
        // but operate on temporary repositories created elsewhere.
        // (The tests could explicitly change directories, but then must be serialized.)
        let repo_path = repo.path().parent().map(Path::to_str).flatten();
        match repo_path {
            Some(path) => {
                command.args(["-C", path]);
            }
            _ => {
                warn!(
                    logger,
                    "Could not determine repository path for rebase. Running in current directory."
                );
            }
        }

        command.args(["rebase", "--interactive", "--autosquash", "--autostash"]);

        for arg in config.rebase_options {
            command.arg(arg);
        }

        if number_of_parents == 0 {
            command.arg("--root");
        } else {
            let base_commit_sha = get_rebase_commit_from_stack(&stack)?;
            let base_commit_sha = commit_id(&base_commit_sha);
            command.arg(&base_commit_sha);
        }

        if config.dry_run {
            info!(logger, "would have run git rebase"; "command" => format!("{:?}", command));
        } else {
            debug!(logger, "running git rebase"; "command" => format!("{:?}", command));
            // Don't check that we have successfully absorbed everything, nor git's
            // exit code -- as git will print helpful messages on its own.
            command.status().expect("could not run git rebase");
        }
    }

    Ok(())
}

struct HunkWithCommit<'c, 'r, 'p> {
    hunk_to_apply: owned::Hunk,
    dest_commit: &'c git2::Commit<'r>,
    index_patch: &'p owned::Patch,
}

fn apply_hunk_to_tree<'repo>(
    repo: &'repo git2::Repository,
    base: &git2::Tree,
    hunk: &owned::Hunk,
    path: &[u8],
) -> Result<git2::Tree<'repo>> {
    let mut treebuilder = repo.treebuilder(Some(base))?;

    // recurse into nested tree if applicable
    if let Some(slash) = path.iter().position(|&x| x == b'/') {
        let (first, rest) = path.split_at(slash);
        let rest = &rest[1..];

        let (subtree, submode) = {
            let entry = treebuilder
                .get(first)?
                .ok_or_else(|| anyhow!("couldn't find tree entry in tree for path"))?;
            (repo.find_tree(entry.id())?, entry.filemode())
        };
        // TODO: loop instead of recursing to avoid potential stack overflow
        let result_subtree = apply_hunk_to_tree(repo, &subtree, hunk, rest)?;

        treebuilder.insert(first, result_subtree.id(), submode)?;
        return Ok(repo.find_tree(treebuilder.write()?)?);
    }

    let (blob, mode) = {
        let entry = treebuilder
            .get(path)?
            .ok_or_else(|| anyhow!("couldn't find blob entry in tree for path"))?;
        (repo.find_blob(entry.id())?, entry.filemode())
    };

    // TODO: convert path to OsStr and pass it during blob_writer
    // creation, to get gitattributes handling (note that converting
    // &[u8] to &std::path::Path is only possible on unixy platforms)
    let mut blobwriter = repo.blob_writer(None)?;
    let old_content = blob.content();
    let (old_start, _, _, _) = hunk.anchors();

    // first, write the lines from the old content that are above the
    // hunk
    let old_content = {
        let (pre, post) = split_lines_after(old_content, old_start);
        blobwriter.write_all(pre)?;
        post
    };
    // next, write the added side of the hunk
    for line in &*hunk.added.lines {
        blobwriter.write_all(line)?;
    }
    // if this hunk removed lines from the old content, those must be
    // skipped
    let (_, old_content) = split_lines_after(old_content, hunk.removed.lines.len());
    // finally, write the remaining lines of the old content
    blobwriter.write_all(old_content)?;

    treebuilder.insert(path, blobwriter.commit()?, mode)?;
    Ok(repo.find_tree(treebuilder.write()?)?)
}

/// Return slices for lines [1..n] and [n+1; ...]
fn split_lines_after(content: &[u8], n: usize) -> (&[u8], &[u8]) {
    let split_index = if n > 0 {
        memchr::Memchr::new(b'\n', content)
            .fuse() // TODO: is fuse necessary here?
            .nth(n - 1) // the position of '\n' ending the `n`-th line
            .map(|x| x + 1)
            .unwrap_or_else(|| content.len())
    } else {
        0
    };
    content.split_at(split_index)
}

fn nothing_left_in_index(repo: &git2::Repository) -> Result<bool> {
    let stats = index_stats(repo)?;
    let nothing = stats.files_changed() == 0 && stats.insertions() == 0 && stats.deletions() == 0;
    Ok(nothing)
}

fn index_stats(repo: &git2::Repository) -> Result<git2::DiffStats> {
    let head = repo.head()?.peel_to_tree()?;
    let diff = repo.diff_tree_to_index(Some(&head), Some(&repo.index()?), None)?;
    let stats = diff.stats()?;
    Ok(stats)
}

fn get_rebase_commit_from_stack<'a>(stack: &WorkingStack<'a>) -> Result<git2::Commit<'a>> {
    // unwrap() is safe here, as we exit early if the stack is empty
    let last_commit_in_stack = &stack.last().unwrap().0;
    // Use a range that is guaranteed to include all the commits we might have
    // committed "fixup!" commits for.
    let rebase_commit = last_commit_in_stack.parent(0)?;
    Ok(rebase_commit)
}

fn commit_id(commit: &git2::Commit) -> String {
    commit.id().to_string()
}

#[cfg(test)]
mod tests {
    use git2::message_trailers_strs;
    use std::path::PathBuf;

    use super::*;
    mod repo_utils;

    #[test]
    fn multiple_fixups_per_commit() {
        let ctx = repo_utils::prepare_and_stage();

        let actual_pre_absorb_commit = ctx.repo.head().unwrap().peel_to_commit().unwrap().id();

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 3);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());

        let pre_absorb_ref_commit = ctx.repo.refname_to_id("PRE_ABSORB_HEAD").unwrap();
        assert_eq!(pre_absorb_ref_commit, actual_pre_absorb_commit);
    }

    #[test]
    fn one_fixup_per_commit() {
        let ctx = repo_utils::prepare_and_stage();

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            one_fixup_per_commit: true,
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 2);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn foreign_author() {
        let ctx = repo_utils::prepare_and_stage();

        repo_utils::become_author(&ctx.repo, "nobody2", "nobody2@example.com");

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);
        let is_something_in_index = !nothing_left_in_index(&ctx.repo).unwrap();
        assert!(is_something_in_index);
    }

    #[test]
    fn foreign_author_with_force_author_flag() {
        let ctx = repo_utils::prepare_and_stage();

        repo_utils::become_author(&ctx.repo, "nobody2", "nobody2@example.com");

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            force_author: true,
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 3);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn foreign_author_with_force_author_config() {
        let ctx = repo_utils::prepare_and_stage();

        repo_utils::become_author(&ctx.repo, "nobody2", "nobody2@example.com");

        repo_utils::set_config_flag(&ctx.repo, "absorb.forceAuthor");

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 3);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn detached_head() {
        let ctx = repo_utils::prepare_and_stage();
        repo_utils::detach_head(&ctx.repo);

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let result = run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo);
        assert_eq!(
            result.err().unwrap().to_string(),
            "HEAD is not a branch, use --force-detach to override"
        );

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);
        let is_something_in_index = !nothing_left_in_index(&ctx.repo).unwrap();
        assert!(is_something_in_index);
    }

    #[test]
    fn detached_head_pointing_at_branch_with_force_detach_flag() {
        let ctx = repo_utils::prepare_and_stage();
        repo_utils::detach_head(&ctx.repo);

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            force_detach: true,
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();
        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();

        assert_eq!(revwalk.count(), 1); // nothing was committed
        let is_something_in_index = !nothing_left_in_index(&ctx.repo).unwrap();
        assert!(is_something_in_index);
    }

    #[test]
    fn detached_head_with_force_detach_flag() {
        let ctx = repo_utils::prepare_and_stage();
        repo_utils::detach_head(&ctx.repo);
        repo_utils::delete_branch(&ctx.repo, "master");

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            force_detach: true,
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();
        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();

        assert_eq!(revwalk.count(), 3);
        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn detached_head_with_force_detach_config() {
        let ctx = repo_utils::prepare_and_stage();
        repo_utils::detach_head(&ctx.repo);
        repo_utils::delete_branch(&ctx.repo, "master");

        repo_utils::set_config_flag(&ctx.repo, "absorb.forceDetach");

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();
        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();

        assert_eq!(revwalk.count(), 3);
        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn and_rebase_flag() {
        let ctx = repo_utils::prepare_and_stage();
        repo_utils::set_config_option(&ctx.repo, "core.editor", "true");
        repo_utils::set_config_option(&ctx.repo, "advice.waitingForEditor", "false");

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            and_rebase: true,
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();

        assert_eq!(revwalk.count(), 1);
        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn and_rebase_flag_with_rebase_options() {
        let ctx = repo_utils::prepare_and_stage();
        repo_utils::set_config_option(&ctx.repo, "core.editor", "true");
        repo_utils::set_config_option(&ctx.repo, "advice.waitingForEditor", "false");

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            and_rebase: true,
            rebase_options: &vec!["--signoff"],
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);

        let trailers = message_trailers_strs(
            ctx.repo
                .head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .message()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            trailers
                .iter()
                .filter(|trailer| trailer.0 == "Signed-off-by")
                .count(),
            1
        );

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn rebase_options_without_and_rebase_flag() {
        let ctx = repo_utils::prepare_and_stage();

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            rebase_options: &vec!["--some-option"],
            ..DEFAULT_CONFIG
        };
        let result = run_with_repo(&logger, &config, &ctx.repo);

        assert_eq!(
            result.err().unwrap().to_string(),
            "REBASE_OPTIONS were specified without --and-rebase flag"
        );

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);
        let is_something_in_index = !nothing_left_in_index(&ctx.repo).unwrap();
        assert!(is_something_in_index);
    }

    #[test]
    fn dry_run_flag() {
        let ctx = repo_utils::prepare_and_stage();

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            dry_run: true,
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);
        let is_something_in_index = !nothing_left_in_index(&ctx.repo).unwrap();
        assert!(is_something_in_index);

        let pre_absorb_ref_commit = ctx.repo.references_glob("PRE_ABSORB_HEAD").unwrap().last();
        assert!(matches!(pre_absorb_ref_commit, None));
    }

    #[test]
    fn dry_run_flag_with_and_rebase_flag() {
        let (ctx, path) = repo_utils::prepare_repo();
        repo_utils::set_config_option(&ctx.repo, "core.editor", "true");

        // create a fixup commit that 'git rebase' will act on if called
        let tree = repo_utils::stage_file_changes(&ctx, &path);
        let signature = ctx.repo.signature().unwrap();
        let head_commit = ctx.repo.head().unwrap().peel_to_commit().unwrap();
        ctx.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                &format!("fixup! {}\n", head_commit.id()),
                &tree,
                &[&head_commit],
            )
            .unwrap();

        // stage one more change so 'git-absorb' won't exit early
        repo_utils::stage_file_changes(&ctx, &path);

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            and_rebase: true,
            dry_run: true,
            ..DEFAULT_CONFIG
        };
        run_with_repo(&logger, &config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 2); // git rebase wasn't called so both commits persist
        let is_something_in_index = !nothing_left_in_index(&ctx.repo).unwrap();
        assert!(is_something_in_index);
    }

    fn autostage_common(ctx: &repo_utils::Context, file_path: &PathBuf) -> (PathBuf, PathBuf) {
        // 1 modification w/o staging
        let path = ctx.join(&file_path);
        let contents = std::fs::read_to_string(&path).unwrap();
        let modifications = format!("{contents}\nnew_line2");
        std::fs::write(&path, &modifications).unwrap();

        // 1 extra file
        let fp2 = PathBuf::from("unrel.txt");
        std::fs::write(ctx.join(&fp2), "foo").unwrap();

        (path, fp2)
    }

    #[test]
    fn autostage_if_index_was_empty() {
        let (ctx, file_path) = repo_utils::prepare_repo();

        // requires enabled config var
        ctx.repo
            .config()
            .unwrap()
            .set_bool(config::AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME, true)
            .unwrap();

        autostage_common(&ctx, &file_path);

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 2);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn do_not_autostage_if_index_was_not_empty() {
        let (ctx, file_path) = repo_utils::prepare_repo();

        // enable config var
        ctx.repo
            .config()
            .unwrap()
            .set_bool(config::AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME, true)
            .unwrap();

        let (_, fp2) = autostage_common(&ctx, &file_path);
        // we stage the extra file - should stay in index
        repo_utils::add(&ctx.repo, &fp2);

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);

        assert_eq!(index_stats(&ctx.repo).unwrap().files_changed(), 1);
    }

    #[test]
    fn do_not_autostage_if_not_enabled_by_config_var() {
        let (ctx, file_path) = repo_utils::prepare_repo();

        // disable config var
        ctx.repo
            .config()
            .unwrap()
            .set_bool(config::AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME, false)
            .unwrap();

        autostage_common(&ctx, &file_path);

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn fixup_message_always_commit_sha_if_configured() {
        let ctx = repo_utils::prepare_and_stage();

        ctx.repo
            .config()
            .unwrap()
            .set_bool(config::FIXUP_TARGET_ALWAYS_SHA_CONFIG_NAME, true)
            .unwrap();

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        run_with_repo(&logger, &DEFAULT_CONFIG, &ctx.repo).unwrap();
        assert!(nothing_left_in_index(&ctx.repo).unwrap());

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();

        let oids: Vec<git2::Oid> = revwalk.by_ref().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(oids.len(), 3);

        let commit = ctx.repo.find_commit(oids[0]).unwrap();
        let actual_msg = commit.summary().unwrap();
        let expected_msg = format!("fixup! {}", oids.last().unwrap());
        assert_eq!(actual_msg, expected_msg);
    }

    const DEFAULT_CONFIG: Config = Config {
        dry_run: false,
        force_author: false,
        force_detach: false,
        base: None,
        and_rebase: false,
        rebase_options: &Vec::new(),
        whole_file: false,
        one_fixup_per_commit: false,
    };
}
