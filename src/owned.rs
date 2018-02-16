extern crate failure;
extern crate git2;

pub fn parse_diff(diff: &git2::Diff) -> Result<Vec<OwnedPatch>, failure::Error> {
    let mut ret = Vec::new();
    for (delta_idx, _delta) in diff.deltas().enumerate() {
        ret.push(OwnedPatch::new(&mut git2::Patch::from_diff(
            diff,
            delta_idx,
        )?.ok_or_else(|| {
            failure::err_msg("got empty delta")
        })?)?);
    }
    Ok(ret)
}

#[derive(Debug)]
pub struct OwnedBlock {
    pub start: u32,
    pub lines: Vec<Vec<u8>>,
    pub trailing_newline: bool,
}
#[derive(Debug)]
pub struct OwnedHunk {
    added: OwnedBlock,
    removed: OwnedBlock,
}
impl OwnedHunk {
    pub fn new(patch: &mut git2::Patch, idx: usize) -> Result<OwnedHunk, failure::Error> {
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
                        .ok_or_else(|| failure::err_msg("added line did not have lineno"))?
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
                        .ok_or_else(|| failure::err_msg("removed line did not have lineno"))?
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
pub struct OwnedPatch {
    old_path: Option<Vec<u8>>,
    old_id: git2::Oid,
    new_path: Option<Vec<u8>>,
    new_id: git2::Oid,
    status: git2::Delta,
    hunks: Vec<OwnedHunk>,
}
impl OwnedPatch {
    pub fn new(patch: &mut git2::Patch) -> Result<OwnedPatch, failure::Error> {
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
