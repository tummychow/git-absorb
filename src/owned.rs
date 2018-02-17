extern crate failure;
extern crate git2;

pub fn parse_diff(diff: &git2::Diff) -> Result<Vec<Patch>, failure::Error> {
    let mut ret = Vec::new();
    for (delta_idx, _delta) in diff.deltas().enumerate() {
        ret.push(Patch::new(&mut git2::Patch::from_diff(diff, delta_idx)?
            .ok_or_else(|| failure::err_msg("got empty delta"))?)?);
    }
    Ok(ret)
}

#[derive(Debug)]
pub struct Block {
    pub start: usize,
    pub lines: Vec<Vec<u8>>,
    pub trailing_newline: bool,
}
#[derive(Debug)]
pub struct Hunk {
    pub added: Block,
    pub removed: Block,
}
impl Hunk {
    pub fn new(patch: &mut git2::Patch, idx: usize) -> Result<Hunk, failure::Error> {
        let mut ret = {
            let (hunk, _size) = patch.hunk(idx)?;
            Hunk {
                added: Block {
                    start: hunk.new_start() as usize,
                    lines: Vec::with_capacity(hunk.new_lines() as usize),
                    trailing_newline: true,
                },
                removed: Block {
                    start: hunk.old_start() as usize,
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
                        as usize != ret.added.start + ret.added.lines.len()
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
                        as usize
                        != ret.removed.start + ret.removed.lines.len()
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
pub struct Patch {
    pub old_path: Option<Vec<u8>>,
    pub old_id: git2::Oid,
    pub new_path: Option<Vec<u8>>,
    pub new_id: git2::Oid,
    pub status: git2::Delta,
    pub hunks: Vec<Hunk>,
}
impl Patch {
    pub fn new(patch: &mut git2::Patch) -> Result<Patch, failure::Error> {
        let mut ret = Patch {
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
            ret.hunks.push(Hunk::new(patch, idx)?);
        }

        Ok(ret)
    }
}
