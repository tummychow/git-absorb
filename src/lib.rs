#[macro_use]
extern crate slog;
use anyhow::{anyhow, Result};

mod commute;
mod config;
mod owned;
mod stack;

use std::io::Write;

pub struct Config<'a> {
    pub dry_run: bool,
    pub force: bool,
    pub base: Option<&'a str>,
    pub and_rebase: bool,
    pub whole_file: bool,
    pub one_fixup_per_commit: bool,
    pub logger: &'a slog::Logger,
}

pub fn run(config: &mut Config) -> Result<()> {
    let repo = git2::Repository::open_from_env()?;
    debug!(config.logger, "repository found"; "path" => repo.path().to_str());

    // here, we default to the git config value,
    // if the flag was not provided in the CLI.
    //
    // in the future, we'd likely want to differentiate between
    // a "non-provided" option, vs an explicit --no-<option>
    // that disables a behavior, much like git does.
    // e.g. user may want to overwrite a config value with
    // --no-one-fixup-per-commit -- then, defaulting to the config value
    // like we do here is no longer sufficient. but until then, this is fine.
    //
    config.one_fixup_per_commit |= config::one_fixup_per_commit(&repo);

    run_with_repo(config, &repo)
}

fn run_with_repo(config: &Config, repo: &git2::Repository) -> Result<()> {
    let stack = stack::working_stack(&repo, config.base, config.force, config.logger)?;
    if stack.is_empty() {
        crit!(config.logger, "No commits available to fix up, exiting");
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
            trace!(config.logger, "parsed commit diff";
                   "commit" => commit.id().to_string(),
                   "diff" => format!("{:?}", diff),
            );
            diffs.push(diff);
        }

        let summary_counts = stack::summary_counts(&stack);
        (
            stack.into_iter().zip(diffs.into_iter()).collect(),
            summary_counts,
        )
    };

    let mut head_tree = repo.head()?.peel_to_tree()?;
    let index = owned::Diff::new(&repo.diff_tree_to_index(
        Some(&head_tree),
        None,
        diff_options.as_mut(),
    )?)?;
    trace!(config.logger, "parsed index";
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
            debug!(config.logger, "skipped non-modified hunk";
                    "path" => String::from_utf8_lossy(old_path).into_owned(),
                    "status" => format!("{:?}", index_patch.status),
            );
            continue 'patch;
        }

        patches_considered += 1;

        let mut preceding_hunks_offset = 0isize;
        let mut applied_hunks_offset = 0isize;
        'hunk: for index_hunk in &index_patch.hunks {
            debug!(config.logger, "next hunk";
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

            debug!(config.logger, "";
                "to apply" => hunk_to_apply.header(),
                "to commute" => isolated_hunk.header(),
                "preceding hunks" => format!("{}/{}", applied_hunks_offset, preceding_hunks_offset),
            );

            preceding_hunks_offset += hunk_offset;

            // find the newest commit that the hunk cannot commute with
            let mut dest_commit = None;
            let mut commuted_old_path = old_path;
            let mut commuted_index_hunk = isolated_hunk;

            'commit: for &(ref commit, ref diff) in &stack {
                let c_logger = config.logger.new(o!(
                    "commit" => commit.id().to_string(),
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
                        config.logger,
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

    hunks_with_commit.sort_by_key(|h| h.dest_commit.id());
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
            &repo,
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
            let dest_commit_id = current.dest_commit.id().to_string();
            let dest_commit_locator = current
                .dest_commit
                .summary()
                .filter(|&msg| summary_counts[msg] == 1)
                .unwrap_or(&dest_commit_id);
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
                info!(config.logger, "committed";
                      "commit" => head_commit.id().to_string(),
                      "header" => format!("+{},-{}", diff.insertions(), diff.deletions()),
                );
            } else {
                info!(config.logger, "would have committed";
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
                config.logger,
                "No changes staged, try adding something \
                 to the index or set {} = true",
                config::AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME
            );
        } else {
            warn!(
                config.logger,
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
        command.args(&["rebase", "--interactive", "--autosquash", "--autostash"]);

        if number_of_parents == 0 {
            command.arg("--root");
        } else {
            // Use a range that is guaranteed to include all the commits we might have
            // committed "fixup!" commits for.
            let base_commit_sha = last_commit_in_stack.parent(0)?.id().to_string();
            command.arg(&base_commit_sha);
        }

        // Don't check that we have successfully absorbed everything, nor git's
        // exit code -- as git will print helpful messages on its own.
        command.status().expect("could not run git rebase");
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    struct Context {
        repo: git2::Repository,
        dir: tempfile::TempDir,
    }

    impl Context {
        fn join(&self, p: &Path) -> PathBuf {
            self.dir.path().join(p)
        }
    }

    /// Prepare a fresh git repository with an initial commit and a file.
    fn prepare_repo() -> (Context, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        let path = PathBuf::from("test-file.txt");
        std::fs::write(
            dir.path().join(&path),
            br#"
line
line

more
lines
"#,
        )
        .unwrap();

        // make the borrow-checker happy by introducing a new scope
        {
            let tree = add(&repo, &path);
            let signature = repo
                .signature()
                .or_else(|_| git2::Signature::now("nobody", "nobody@example.com"))
                .unwrap();
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Initial commit.",
                &tree,
                &[],
            )
            .unwrap();
        }

        (Context { repo, dir }, path)
    }

    /// Stage the changes made to `path`.
    fn add<'r>(repo: &'r git2::Repository, path: &Path) -> git2::Tree<'r> {
        let mut index = repo.index().unwrap();
        index.add_path(&path).unwrap();
        index.write().unwrap();

        let tree_id = index.write_tree_to(&repo).unwrap();
        repo.find_tree(tree_id).unwrap()
    }

    /// Prepare an empty repo, and stage some changes.
    fn prepare_and_stage() -> Context {
        let (ctx, file_path) = prepare_repo();

        // add some lines to our file
        let path = ctx.join(&file_path);
        let contents = std::fs::read_to_string(&path).unwrap();
        let modifications = format!("new_line1\n{contents}\nnew_line2");
        std::fs::write(&path, &modifications).unwrap();

        // stage it
        add(&ctx.repo, &file_path);

        ctx
    }

    #[test]
    fn multiple_fixups_per_commit() {
        let ctx = prepare_and_stage();

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            dry_run: false,
            force: false,
            base: None,
            and_rebase: false,
            whole_file: false,
            one_fixup_per_commit: false,
            logger: &logger,
        };
        run_with_repo(&config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 3);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn one_fixup_per_commit() {
        let ctx = prepare_and_stage();

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            dry_run: false,
            force: false,
            base: None,
            and_rebase: false,
            whole_file: false,
            one_fixup_per_commit: true,
            logger: &logger,
        };
        run_with_repo(&config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 2);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    fn autostage_common(ctx: &Context, file_path: &PathBuf) -> (PathBuf, PathBuf) {
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
        let (ctx, file_path) = prepare_repo();

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
        let config = Config {
            dry_run: false,
            force: false,
            base: None,
            and_rebase: false,
            whole_file: false,
            one_fixup_per_commit: false,
            logger: &logger,
        };
        run_with_repo(&config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 2);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }

    #[test]
    fn do_not_autostage_if_index_was_not_empty() {
        let (ctx, file_path) = prepare_repo();

        // enable config var
        ctx.repo
            .config()
            .unwrap()
            .set_bool(config::AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME, true)
            .unwrap();

        let (_, fp2) = autostage_common(&ctx, &file_path);
        // we stage the extra file - should stay in index
        add(&ctx.repo, &fp2);

        // run 'git-absorb'
        let drain = slog::Discard;
        let logger = slog::Logger::root(drain, o!());
        let config = Config {
            dry_run: false,
            force: false,
            base: None,
            and_rebase: false,
            whole_file: false,
            one_fixup_per_commit: false,
            logger: &logger,
        };
        run_with_repo(&config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);

        assert_eq!(index_stats(&ctx.repo).unwrap().files_changed(), 1);
    }

    #[test]
    fn do_not_autostage_if_not_enabled_by_config_var() {
        let (ctx, file_path) = prepare_repo();

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
        let config = Config {
            dry_run: false,
            force: false,
            base: None,
            and_rebase: false,
            whole_file: false,
            one_fixup_per_commit: false,
            logger: &logger,
        };
        run_with_repo(&config, &ctx.repo).unwrap();

        let mut revwalk = ctx.repo.revwalk().unwrap();
        revwalk.push_head().unwrap();
        assert_eq!(revwalk.count(), 1);

        assert!(nothing_left_in_index(&ctx.repo).unwrap());
    }
}
