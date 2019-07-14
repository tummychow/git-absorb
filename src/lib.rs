extern crate failure;
extern crate git2;
extern crate memchr;
#[macro_use]
extern crate slog;

mod commute;
mod owned;
mod stack;

use std::io::Write;

pub struct Config<'a> {
    pub dry_run: bool,
    pub force: bool,
    pub base: Option<&'a str>,
    pub logger: &'a slog::Logger,
}

pub fn run(config: &Config) -> Result<(), failure::Error> {
    let repo = git2::Repository::open_from_env()?;
    debug!(config.logger, "repository found"; "path" => repo.path().to_str());

    let mut diff_options = Some({
        let mut ret = git2::DiffOptions::new();
        ret.context_lines(0)
            .id_abbrev(40)
            .ignore_filemode(true)
            .ignore_submodules(true);
        ret
    });

    let (stack, summary_counts): (Vec<_>, _) = {
        let stack = stack::working_stack(&repo, config.base, config.logger)?;
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

    let signature = repo.signature()?;
    let mut head_commit = repo.head()?.peel_to_commit()?;

    'patch: for index_patch in index.iter() {
        let old_path = index_patch.new_path.as_slice();
        if index_patch.status != git2::Delta::Modified {
            debug!(config.logger, "skipped non-modified hunk";
                    "path" => String::from_utf8_lossy(old_path).into_owned(),
                    "status" => format!("{:?}", index_patch.status),
            );
            continue 'patch;
        }

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

            // To aid in understanding these arithmetics, here's an illustration.
            // There are two hunks in the original patch, each adding one line ("line2" and
            // "line5"). Assuming the first hunk (with offset = -1) was already proceesed
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
                    debug!(config.logger, "could not find noncommutative commit");
                    continue 'hunk;
                }
            };

            // TODO: the git2 api only supports utf8 commit messages,
            // so it's okay to use strings instead of bytes here
            // https://docs.rs/git2/0.7.5/src/git2/repo.rs.html#998
            // https://libgit2.org/libgit2/#HEAD/group/commit/git_commit_create
            let dest_commit_id = dest_commit.id().to_string();
            let dest_commit_locator = dest_commit
                .summary()
                .filter(|&msg| summary_counts[msg] == 1)
                .unwrap_or(&dest_commit_id);
            if !config.dry_run {
                head_tree =
                    apply_hunk_to_tree(&repo, &head_tree, &hunk_to_apply, &index_patch.old_path)?;
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
                      "header" => hunk_to_apply.header(),
                );
            } else {
                info!(config.logger, "would have committed";
                      "fixup" => dest_commit_locator,
                      "header" => hunk_to_apply.header(),
                );
            }
            applied_hunks_offset += hunk_offset;
        }
    }

    Ok(())
}

fn apply_hunk_to_tree<'repo>(
    repo: &'repo git2::Repository,
    base: &git2::Tree,
    hunk: &owned::Hunk,
    path: &[u8],
) -> Result<git2::Tree<'repo>, failure::Error> {
    let mut treebuilder = repo.treebuilder(Some(base))?;

    // recurse into nested tree if applicable
    if let Some(slash) = path.iter().position(|&x| x == b'/') {
        let (first, rest) = path.split_at(slash);
        let rest = &rest[1..];

        let (subtree, submode) = {
            let entry = treebuilder
                .get(first)?
                .ok_or_else(|| failure::err_msg("couldn't find tree entry in tree for path"))?;
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
            .ok_or_else(|| failure::err_msg("couldn't find blob entry in tree for path"))?;
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
