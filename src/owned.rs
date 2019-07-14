extern crate failure;
extern crate git2;

use std::collections::hash_map::HashMap;
use std::rc::Rc;

#[derive(Debug)]
pub struct Diff {
    patches: Vec<Patch>,
    by_new: HashMap<Vec<u8>, usize>,
    by_old: HashMap<Vec<u8>, usize>,
}
impl ::std::ops::Deref for Diff {
    type Target = [Patch];
    fn deref(&self) -> &[Patch] {
        self.patches.as_slice()
    }
}
impl Diff {
    pub fn new(diff: &git2::Diff) -> Result<Self, failure::Error> {
        let mut ret = Diff {
            patches: Vec::new(),
            by_old: HashMap::new(),
            by_new: HashMap::new(),
        };

        for (delta_idx, _delta) in diff.deltas().enumerate() {
            let patch = Patch::new(
                &mut git2::Patch::from_diff(diff, delta_idx)?
                    .ok_or_else(|| failure::err_msg("got empty delta"))?,
            )?;
            if ret.by_old.contains_key(&patch.old_path) {
                // TODO: would this case be hit if the diff was put through copy detection?
                return Err(failure::err_msg("old path already occupied"));
            }
            ret.by_old.insert(patch.old_path.clone(), ret.patches.len());
            if ret.by_new.contains_key(&patch.new_path) {
                return Err(failure::err_msg("new path already occupied"));
            }
            ret.by_new.insert(patch.new_path.clone(), ret.patches.len());
            ret.patches.push(patch);
        }

        Ok(ret)
    }
    pub fn by_old(&self, path: &[u8]) -> Option<&Patch> {
        self.by_old.get(path).map(|&idx| &self.patches[idx])
    }
    pub fn by_new(&self, path: &[u8]) -> Option<&Patch> {
        self.by_new.get(path).map(|&idx| &self.patches[idx])
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub start: usize,
    pub lines: Rc<Vec<Vec<u8>>>,
    pub trailing_newline: bool,
}
#[derive(Debug, Clone)]
pub struct Hunk {
    pub added: Block,
    pub removed: Block,
}
impl Hunk {
    pub fn new(patch: &mut git2::Patch, idx: usize) -> Result<Self, failure::Error> {
        let (added_start, removed_start, mut added_lines, mut removed_lines) = {
            let (hunk, _size) = patch.hunk(idx)?;
            (
                hunk.new_start() as usize,
                hunk.old_start() as usize,
                Vec::with_capacity(hunk.new_lines() as usize),
                Vec::with_capacity(hunk.old_lines() as usize),
            )
        };
        let mut added_trailing_newline = true;
        let mut removed_trailing_newline = true;

        for line_idx in 0..patch.num_lines_in_hunk(idx)? {
            let line = patch.line_in_hunk(idx, line_idx)?;
            match line.origin() {
                '+' => {
                    if line.num_lines() > 1 {
                        return Err(failure::err_msg("wrong number of lines in hunk"));
                    }
                    if line
                        .new_lineno()
                        .ok_or_else(|| failure::err_msg("added line did not have lineno"))?
                        as usize
                        != added_start + added_lines.len()
                    {
                        return Err(failure::err_msg("added line did not reach expected lineno"));
                    }
                    added_lines.push(Vec::from(line.content()))
                }
                '-' => {
                    if line.num_lines() > 1 {
                        return Err(failure::err_msg("wrong number of lines in hunk"));
                    }
                    if line
                        .old_lineno()
                        .ok_or_else(|| failure::err_msg("removed line did not have lineno"))?
                        as usize
                        != removed_start + removed_lines.len()
                    {
                        return Err(failure::err_msg(
                            "removed line did not reach expected lineno",
                        ));
                    }
                    removed_lines.push(Vec::from(line.content()))
                }
                '>' => {
                    if !removed_trailing_newline {
                        return Err(failure::err_msg("removed nneof was already detected"));
                    };
                    removed_trailing_newline = false
                }
                '<' => {
                    if !added_trailing_newline {
                        return Err(failure::err_msg("added nneof was already detected"));
                    };
                    added_trailing_newline = false
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
            if added_lines.len() != hunk.new_lines() as usize {
                return Err(failure::err_msg("hunk added block size mismatch"));
            }
            if removed_lines.len() != hunk.old_lines() as usize {
                return Err(failure::err_msg("hunk removed block size mismatch"));
            }
        }

        Ok(Hunk {
            added: Block {
                start: added_start,
                lines: Rc::new(added_lines),
                trailing_newline: added_trailing_newline,
            },
            removed: Block {
                start: removed_start,
                lines: Rc::new(removed_lines),
                trailing_newline: removed_trailing_newline,
            },
        })
    }

    /// Returns the unchanged lines around this hunk.
    ///
    /// Any given hunk has four anchor points:
    ///
    /// - the last unchanged line before it, on the removed side
    /// - the first unchanged line after it, on the removed side
    /// - the last unchanged line before it, on the added side
    /// - the first unchanged line after it, on the added side
    ///
    /// This function returns those four line numbers, in that order.
    pub fn anchors(&self) -> (usize, usize, usize, usize) {
        match (self.removed.lines.len(), self.added.lines.len()) {
            (0, 0) => (0, 1, 0, 1),
            (removed_len, 0) => (
                self.removed.start - 1,
                self.removed.start + removed_len,
                self.removed.start - 1,
                self.removed.start,
            ),
            (0, added_len) => (
                self.added.start - 1,
                self.added.start,
                self.added.start - 1,
                self.added.start + added_len,
            ),
            (removed_len, added_len) => (
                self.removed.start - 1,
                self.removed.start + removed_len,
                self.added.start - 1,
                self.added.start + added_len,
            ),
        }
    }

    pub fn changed_offset(&self) -> isize {
        self.added.lines.len() as isize - self.removed.lines.len() as isize
    }

    pub fn header(&self) -> String {
        format!(
            "-{},{} +{},{}",
            self.removed.start,
            self.removed.lines.len(),
            self.added.start,
            self.added.lines.len()
        )
    }

    pub fn shift_added_block(mut self, by: isize) -> Self {
        self.added.start = (self.added.start as isize + by) as usize;
        self
    }

    pub fn shift_both_blocks(mut self, by: isize) -> Self {
        self.removed.start = (self.removed.start as isize + by) as usize;
        self.added.start = (self.added.start as isize + by) as usize;
        self
    }
}

#[derive(Debug)]
pub struct Patch {
    pub old_path: Vec<u8>,
    pub old_id: git2::Oid,
    pub new_path: Vec<u8>,
    pub new_id: git2::Oid,
    pub status: git2::Delta,
    pub hunks: Vec<Hunk>,
}
impl Patch {
    pub fn new(patch: &mut git2::Patch) -> Result<Self, failure::Error> {
        let mut ret = Patch {
            old_path: patch
                .delta()
                .old_file()
                .path_bytes()
                .map(Vec::from)
                .ok_or_else(|| failure::err_msg("delta with empty old path"))?,
            old_id: patch.delta().old_file().id(),
            new_path: patch
                .delta()
                .new_file()
                .path_bytes()
                .map(Vec::from)
                .ok_or_else(|| failure::err_msg("delta with empty new path"))?,
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
