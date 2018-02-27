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
            debug!(config.logger, "parsed commit diff";
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
    debug!(config.logger, "parsed index";
           "index" => format!("{:?}", index),
    );

    'patch: for index_patch in index.iter() {
        'hunk: for index_hunk in &index_patch.hunks {
            let mut commuted_index_hunk = index_hunk.clone();
            let mut commuted_old_path = match index_patch.old_path.as_ref() {
                Some(path) => path,
                // this index patch is for a newly added file, so it
                // can't be absorbed, and the whole patch should be
                // skipped
                None => continue 'patch,
            };

            // find the newest commit that the hunk cannot commute
            // with
            let mut dest_commit = None;
            'commit: for &(ref commit, ref diff) in &stack {
                let next_patch = match diff.by_new(commuted_old_path) {
                    Some(patch) => patch,
                    // this commit doesn't touch the hunk's file, so
                    // they trivially commute, and the next commit
                    // should be considered
                    None => continue 'commit,
                };
                commuted_old_path = match next_patch.old_path.as_ref() {
                    Some(path) => path,
                    // this commit introduced the file that the hunk
                    // is part of, so the hunk cannot commute with it
                    None => {
                        dest_commit = Some(commit);
                        break 'commit;
                    }
                };
                commuted_index_hunk =
                    match commute::commute_diff_before(&commuted_index_hunk, &next_patch.hunks) {
                        Some(hunk) => hunk,
                        // this commit contains a hunk that cannot
                        // commute with the hunk being absorbed
                        None => {
                            dest_commit = Some(commit);
                            break 'commit;
                        }
                    };
            }
            let dest_commit = match dest_commit {
                Some(commit) => commit,
                // the hunk commutes with every commit in the stack,
                // so there is no commit to absorb it into
                None => continue 'hunk,
            };
        }
    }

    Ok(())
}
