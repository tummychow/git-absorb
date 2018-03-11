extern crate failure;
extern crate git2;
#[macro_use]
extern crate slog;

mod owned;
mod stack;
mod commute;

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

    let mut diff_options = Some({
        let mut ret = git2::DiffOptions::new();
        ret.context_lines(0)
            .id_abbrev(40)
            .ignore_filemode(true)
            .ignore_submodules(true);
        ret
    });

    let stack: Vec<_> = {
        let stack = stack::working_stack(&repo, base.as_ref(), config.logger)?;
        let mut diffs = Vec::with_capacity(stack.len());
        for commit in &stack {
            let diff = owned::Diff::new(&repo.diff_tree_to_tree(
                if commit.parents().len() == 0 {
                    None
                } else {
                    Some(commit.parent(0)?.tree()?)
                }.as_ref(),
                Some(&commit.tree()?),
                diff_options.as_mut(),
            )?)?;
            trace!(config.logger, "parsed commit diff";
                   "commit" => commit.id().to_string(),
                   "diff" => format!("{:?}", diff),
            );
            diffs.push(diff);
        }

        stack.into_iter().zip(diffs.into_iter()).collect()
    };

    let index = owned::Diff::new(&repo.diff_tree_to_index(
        Some(&repo.head()?.peel_to_tree()?),
        None,
        diff_options.as_mut(),
    )?)?;
    trace!(config.logger, "parsed index";
           "index" => format!("{:?}", index),
    );

    'patch: for index_patch in index.iter() {
        'hunk: for index_hunk in &index_patch.hunks {
            let mut commuted_index_hunk = index_hunk.clone();
            if index_patch.status != git2::Delta::Modified {
                debug!(config.logger, "skipped non-modified hunk";
                       "path" => String::from_utf8_lossy(index_patch.new_path.as_slice()).into_owned(),
                       "status" => format!("{:?}", index_patch.status),
                );
                continue 'patch;
            }
            let mut commuted_old_path = index_patch.old_path.as_slice();
            debug!(config.logger, "commuting hunk";
                   "path" => String::from_utf8_lossy(commuted_old_path).into_owned(),
                   "header" => format!("-{},{} +{},{}",
                                     commuted_index_hunk.removed.start,
                                     commuted_index_hunk.removed.lines.len(),
                                     commuted_index_hunk.added.start,
                                     commuted_index_hunk.added.lines.len(),
                   ),
            );

            // find the newest commit that the hunk cannot commute
            // with
            let mut dest_commit = None;
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
        }
    }

    Ok(())
}
