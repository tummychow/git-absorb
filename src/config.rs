use crate::Config;
use git2::Repository;

pub const MAX_STACK_CONFIG_NAME: &str = "absorb.maxStack";
pub const MAX_STACK: usize = 10;

pub const FORCE_AUTHOR_CONFIG_NAME: &str = "absorb.forceAuthor";
pub const FORCE_AUTHOR_DEFAULT: bool = false;

pub const ONE_FIXUP_PER_COMMIT_CONFIG_NAME: &str = "absorb.oneFixupPerCommit";
pub const ONE_FIXUP_PER_COMMIT_DEFAULT: bool = false;

pub const AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME: &str = "absorb.autoStageIfNothingStaged";
pub const AUTO_STAGE_IF_NOTHING_STAGED_DEFAULT: bool = false;

pub const FIXUP_TARGET_ALWAYS_SHA_CONFIG_NAME: &str = "absorb.fixupTargetAlwaysSHA";
pub const FIXUP_TARGET_ALWAYS_SHA_DEFAULT: bool = false;

pub fn unify<'config>(config: &'config Config, repo: &Repository) -> Config<'config> {
    Config {
        // here, we default to the git config value,
        // if the flag was not provided in the CLI.
        //
        // in the future, we'd likely want to differentiate between
        // a "non-provided" option, vs an explicit --no-<option>
        // that disables a behavior, much like git does.
        // e.g. user may want to overwrite a config value with
        // --no-one-fixup-per-commit -- then, defaulting to the config value
        // like we do here is no longer sufficient. but until then, this is fine.
        one_fixup_per_commit: config.one_fixup_per_commit || one_fixup_per_commit(&repo),
        force_author: config.force_author || config.force || force_author(&repo),
        ..*config
    }
}

pub fn max_stack(repo: &git2::Repository) -> usize {
    match repo
        .config()
        .and_then(|config| config.get_i64(MAX_STACK_CONFIG_NAME))
    {
        Ok(max_stack) if max_stack > 0 => max_stack as usize,
        _ => MAX_STACK,
    }
}

fn force_author(repo: &git2::Repository) -> bool {
    match repo
        .config()
        .and_then(|config| config.get_bool(FORCE_AUTHOR_CONFIG_NAME))
    {
        Ok(force_author) => force_author,
        _ => FORCE_AUTHOR_DEFAULT,
    }
}

fn one_fixup_per_commit(repo: &git2::Repository) -> bool {
    match repo
        .config()
        .and_then(|config| config.get_bool(ONE_FIXUP_PER_COMMIT_CONFIG_NAME))
    {
        Ok(one_commit_per_fixup) => one_commit_per_fixup,
        _ => ONE_FIXUP_PER_COMMIT_DEFAULT,
    }
}

pub fn auto_stage_if_nothing_staged(repo: &git2::Repository) -> bool {
    match repo
        .config()
        .and_then(|config| config.get_bool(AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME))
    {
        Ok(val) => val,
        _ => AUTO_STAGE_IF_NOTHING_STAGED_DEFAULT,
    }
}

pub fn fixup_target_always_sha(repo: &git2::Repository) -> bool {
    match repo
        .config()
        .and_then(|config| config.get_bool(FIXUP_TARGET_ALWAYS_SHA_CONFIG_NAME))
    {
        Ok(val) => val,
        _ => FIXUP_TARGET_ALWAYS_SHA_DEFAULT,
    }
}
