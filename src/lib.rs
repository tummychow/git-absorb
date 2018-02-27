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

    Ok(())
}
