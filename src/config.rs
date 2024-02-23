pub const MAX_STACK_CONFIG_NAME: &str = "absorb.maxStack";
pub const MAX_STACK: usize = 10;

pub const ONE_FIXUP_PER_COMMIT_CONFIG_NAME: &str = "absorb.oneFixupPerCommit";
pub const ONE_FIXUP_PER_COMMIT_DEFAULT: bool = false;

pub const AUTO_STAGE_IF_NOTHING_STAGED_CONFIG_NAME: &str = "absorb.autoStageIfNothingStaged";
pub const AUTO_STAGE_IF_NOTHING_STAGED_DEFAULT: bool = false;

pub fn max_stack(repo: &git2::Repository) -> usize {
    match repo
        .config()
        .and_then(|config| config.get_i64(MAX_STACK_CONFIG_NAME))
    {
        Ok(max_stack) if max_stack > 0 => max_stack as usize,
        _ => MAX_STACK,
    }
}

pub fn one_fixup_per_commit(repo: &git2::Repository) -> bool {
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
