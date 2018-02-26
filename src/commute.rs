extern crate failure;

use owned;
use std::iter;

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
fn anchors(hunk: &owned::Hunk) -> (usize, usize, usize, usize) {
    match (hunk.removed.lines.len(), hunk.added.lines.len()) {
        (0, 0) => (0, 1, 0, 1),
        (removed_len, 0) => (
            hunk.removed.start - 1,
            hunk.removed.start + removed_len,
            hunk.removed.start - 1,
            hunk.removed.start,
        ),
        (0, added_len) => (
            hunk.added.start - 1,
            hunk.added.start,
            hunk.added.start - 1,
            hunk.added.start + added_len,
        ),
        (removed_len, added_len) => (
            hunk.removed.start - 1,
            hunk.removed.start + removed_len,
            hunk.added.start - 1,
            hunk.added.start + added_len,
        ),
    }
}

/// Tests if all elements of the iterator are equal to each other.
///
/// An empty iterator returns `true`.
///
/// `uniform()` is short-circuiting. It will stop processing as soon
/// as it finds two pairwise inequal elements.
fn uniform<I, E>(mut iter: I) -> bool
where
    I: iter::Iterator<Item = E>,
    E: ::std::cmp::Eq,
{
    match iter.next() {
        Some(first) => iter.all(|e| e == first),
        None => true,
    }
}

pub fn commute(first: &owned::Hunk, second: &owned::Hunk) -> Option<(owned::Hunk, owned::Hunk)> {
    let (_, _, first_upper, first_lower) = anchors(first);
    let (second_upper, second_lower, _, _) = anchors(second);

    // represent hunks in content order rather than application order
    let (first_above, above, below) = {
        if first_lower <= second_upper {
            (true, first, second)
        } else if second_lower <= first_upper {
            (false, second, first)
        } else {
            // if both hunks are exclusively adding or removing, and
            // both hunks are composed entirely of the same line being
            // repeated, then they commute no matter what their
            // offsets are, because they can be interleaved in any
            // order without changing the final result
            if first.added.lines.is_empty() && second.added.lines.is_empty()
                && uniform(first.removed.lines.iter().chain(&*second.removed.lines))
            {
                // TODO: removed start positions probably need to be
                // tweaked here
                return Some((second.clone(), first.clone()));
            } else if first.removed.lines.is_empty() && second.removed.lines.is_empty()
                && uniform(first.added.lines.iter().chain(&*second.added.lines))
            {
                // TODO: added start positions probably need to be
                // tweaked here
                return Some((second.clone(), first.clone()));
            }
            // these hunks overlap and cannot be interleaved, so they
            // do not commute
            return None;
        }
    };

    let above = above.clone();
    let mut below = below.clone();
    let above_change_offset = (above.added.lines.len() as i64 - above.removed.lines.len() as i64)
        * if first_above { -1 } else { 1 };
    below.added.start = (below.added.start as i64 + above_change_offset) as usize;
    below.removed.start = (below.removed.start as i64 + above_change_offset) as usize;

    Some(if first_above {
        (below, above)
    } else {
        (above, below)
    })
}

pub fn commute_diff_before<'a, I>(after: &owned::Hunk, before: I) -> Option<owned::Hunk>
where
    I: iter::IntoIterator<Item = &'a owned::Hunk>,
    <I as iter::IntoIterator>::IntoIter: iter::DoubleEndedIterator,
{
    before
        .into_iter()
        // the patch's hunks must be iterated in reverse application
        // order (last applied to first applied), which also happens
        // to be reverse line order (bottom to top), which also
        // happens to be reverse of the order they're stored
        .rev()
        .fold(Some(after.clone()), |after, next| {
            after
                .and_then(|after| commute(next, &after))
                .map(|(commuted_after, _)| commuted_after)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn test_commute() {
        let hunk1 = owned::Hunk {
            added: owned::Block {
                start: 2,
                lines: Rc::new(vec![b"bar\n".to_vec()]),
                trailing_newline: true,
            },
            removed: owned::Block {
                start: 1,
                lines: Rc::new(vec![]),
                trailing_newline: true,
            },
        };

        let hunk2 = owned::Hunk {
            added: owned::Block {
                start: 1,
                lines: Rc::new(vec![b"bar\n".to_vec()]),
                trailing_newline: true,
            },
            removed: owned::Block {
                start: 0,
                lines: Rc::new(vec![]),
                trailing_newline: true,
            },
        };

        let (new1, new2) = commute(&hunk1, &hunk2).unwrap();
        assert_eq!(new1.added.start, 1);
        assert_eq!(new2.added.start, 3);
    }

    #[test]
    fn test_commute_interleave() {
        let mut line = iter::repeat(b"bar\n".to_vec());
        let hunk1 = owned::Hunk {
            added: owned::Block {
                start: 1,
                lines: Rc::new((&mut line).take(4).collect::<Vec<_>>()),
                trailing_newline: true,
            },
            removed: owned::Block {
                start: 0,
                lines: Rc::new(vec![]),
                trailing_newline: true,
            },
        };
        let hunk2 = owned::Hunk {
            added: owned::Block {
                start: 1,
                lines: Rc::new((&mut line).take(2).collect::<Vec<_>>()),
                trailing_newline: true,
            },
            removed: owned::Block {
                start: 0,
                lines: Rc::new(vec![]),
                trailing_newline: true,
            },
        };

        let (new1, new2) = commute(&hunk1, &hunk2).unwrap();
        assert_eq!(new1.added.lines.len(), 2);
        assert_eq!(new2.added.lines.len(), 4);
    }

    #[test]
    fn test_commute_patch() {
        let patch = vec![
            owned::Hunk {
                added: owned::Block {
                    start: 1,
                    lines: Rc::new(vec![b"bar\n".to_vec()]),
                    trailing_newline: true,
                },
                removed: owned::Block {
                    start: 0,
                    lines: Rc::new(vec![]),
                    trailing_newline: true,
                },
            },
            owned::Hunk {
                added: owned::Block {
                    start: 3,
                    lines: Rc::new(vec![b"bar\n".to_vec()]),
                    trailing_newline: true,
                },
                removed: owned::Block {
                    start: 1,
                    lines: Rc::new(vec![]),
                    trailing_newline: true,
                },
            },
        ];
        let hunk = owned::Hunk {
            added: owned::Block {
                start: 5,
                lines: Rc::new(vec![b"bar\n".to_vec()]),
                trailing_newline: true,
            },
            removed: owned::Block {
                start: 4,
                lines: Rc::new(vec![]),
                trailing_newline: true,
            },
        };

        let commuted = commute_diff_before(&hunk, &patch).unwrap();
        assert_eq!(commuted.added.start, 3);
    }
}
