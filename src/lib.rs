extern crate failure;
extern crate git2;
#[macro_use]
extern crate slog;

mod owned;
mod stack;

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
            let diff = owned::parse_diff(&repo.diff_tree_to_tree(
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

    let index = owned::parse_diff(&repo.diff_tree_to_index(
        Some(&repo.head()?.peel_to_tree()?),
        None,
        diff_options.as_mut(),
    )?)?;
    debug!(config.logger, "parsed index";
           "index" => format!("{:?}", index),
    );

    Ok(())
}

fn overlap(above: &owned::Block, below: &owned::Block) -> bool {
    !above.lines.is_empty() && !below.lines.is_empty()
        && below.start - above.start - above.lines.len() == 0
}

fn commute(
    first: owned::Hunk,
    second: owned::Hunk,
) -> Result<(bool, owned::Hunk, owned::Hunk), failure::Error> {
    // represent hunks in content order rather than application order
    let (first_above, above, mut below) = match (
        first.added.start <= second.added.start,
        first.removed.start <= second.removed.start,
    ) {
        (true, true) => (true, first, second),
        (false, false) => (false, second, first),
        _ => return Err(failure::err_msg("nonsensical hunk ordering")),
    };

    // if the hunks overlap on either side, they can't commute, so return them in original order
    if overlap(&above.added, &below.added) || overlap(&above.removed, &below.removed) {
        return Ok(if first_above {
            (false, above, below)
        } else {
            (false, below, above)
        });
    }

    let above_change_offset = (above.added.lines.len() as i64 - above.removed.lines.len() as i64)
        * if first_above { -1 } else { 1 };
    below.added.start = (below.added.start as i64 + above_change_offset) as usize;
    below.removed.start = (below.removed.start as i64 + above_change_offset) as usize;

    Ok(if first_above {
        (true, below, above)
    } else {
        (true, above, below)
    })
}
