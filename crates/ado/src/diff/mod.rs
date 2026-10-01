//! Changes between two commits, and the line diff that turns two versions
//! of a file into hunks.

pub(crate) mod get;

use std::iter::repeat_n;

#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Same,
    Removed,
    Added,
}

/// One hunk: its unified text, and the lines it changes on each side
/// (first and last, from 1).
pub(crate) struct Hunk {
    pub(crate) text: String,
    pub(crate) added: Option<(usize, usize)>,
    pub(crate) removed: Option<(usize, usize)>,
}

/// A file's hunks with `context` unchanged lines around each change, and how
/// many lines the change adds and removes in all.
pub(crate) struct LineDiff {
    pub(crate) hunks: Vec<Hunk>,
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

/// The largest middle (old lines times new lines, once the common start and
/// end are set aside) diffed line by line.
const MAX_CELLS: usize = 4_000_000;

pub(crate) fn diff(old: &str, new: &str, context: usize) -> LineDiff {
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let edits = script(&old, &new);
    // Where each edit stands: how many old and new lines come before it.
    let mut at = Vec::with_capacity(edits.len());
    let (mut o, mut n) = (0, 0);
    for edit in &edits {
        at.push((o, n));
        match edit {
            Edit::Same => (o, n) = (o + 1, n + 1),
            Edit::Removed => o += 1,
            Edit::Added => n += 1,
        }
    }
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (index, _) in edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| **edit != Edit::Same)
    {
        match groups.last_mut() {
            // Changes at most 2 × context lines apart share a hunk.
            Some((_, last)) if index - *last <= 2 * context + 1 => *last = index,
            _ => groups.push((index, index)),
        }
    }
    let hunks = groups
        .into_iter()
        .map(|(first, last)| {
            let span = first.saturating_sub(context)..=(last + context).min(edits.len() - 1);
            let count = |skip: Edit| span.clone().filter(|k| edits[*k] != skip).count();
            let (old_at, new_at) = at[*span.start()];
            let mut text = format!(
                "@@ -{} +{} @@",
                range(old_at, count(Edit::Added)),
                range(new_at, count(Edit::Removed))
            );
            for k in span.clone() {
                let (sign, line) = match edits[k] {
                    Edit::Same => (' ', old[at[k].0]),
                    Edit::Removed => ('-', old[at[k].0]),
                    Edit::Added => ('+', new[at[k].1]),
                };
                text.push('\n');
                text.push(sign);
                text.push_str(line);
            }
            let lines = |kind: Edit, side: fn((usize, usize)) -> usize| {
                let mut numbers = (first..=last)
                    .filter(|k| edits[*k] == kind)
                    .map(|k| side(at[k]) + 1);
                let start = numbers.next()?;
                Some((start, numbers.next_back().unwrap_or(start)))
            };
            Hunk {
                text,
                added: lines(Edit::Added, |(_, new)| new),
                removed: lines(Edit::Removed, |(old, _)| old),
            }
        })
        .collect();
    LineDiff {
        hunks,
        added: edits.iter().filter(|edit| **edit == Edit::Added).count(),
        removed: edits.iter().filter(|edit| **edit == Edit::Removed).count(),
    }
}

/// `start,length` as a unified header writes it: from 1, the line before
/// when the side is empty, the length left out when it is 1.
fn range(before: usize, length: usize) -> String {
    match length {
        0 => format!("{before},0"),
        1 => format!("{}", before + 1),
        _ => format!("{},{length}", before + 1),
    }
}

/// The edits that turn `old` into `new`, a line at a time: the common start
/// and end, and a longest common subsequence of the middle.
fn script(old: &[&str], new: &[&str]) -> Vec<Edit> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );
    let mut edits: Vec<Edit> = repeat_n(Edit::Same, prefix).collect();
    if a.len().saturating_mul(b.len()) > MAX_CELLS {
        // ponytail: past MAX_CELLS the middle is one replacement, not a
        // minimal diff; Myers' algorithm when such files matter.
        edits.extend(repeat_n(Edit::Removed, a.len()));
        edits.extend(repeat_n(Edit::Added, b.len()));
    } else {
        // lcs[i * width + j]: the longest common subsequence of a[i..] and b[j..].
        let width = b.len() + 1;
        let mut lcs = vec![0_u32; (a.len() + 1) * width];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                lcs[i * width + j] = if a[i] == b[j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                edits.push(Edit::Same);
                (i, j) = (i + 1, j + 1);
            } else if j == b.len()
                || (i < a.len() && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
            {
                edits.push(Edit::Removed);
                i += 1;
            } else {
                edits.push(Edit::Added);
                j += 1;
            }
        }
    }
    edits.extend(repeat_n(Edit::Same, suffix));
    edits
}

#[cfg(test)]
mod tests {
    use super::diff;

    /// Lines 1 to 20, each its number, with `edit` applied.
    fn lines(edit: impl Fn(&mut Vec<String>)) -> String {
        let mut lines: Vec<String> = (1..=20).map(|n| n.to_string()).collect();
        edit(&mut lines);
        lines.join("\n") + "\n"
    }

    #[test]
    fn a_change_is_a_hunk_with_context_and_far_changes_are_two() {
        let old = lines(|_| {});
        let new = lines(|lines| {
            lines[2] = "three".into();
            lines.remove(16);
            lines.insert(17, "18b".into());
        });
        let got = diff(&old, &new, 2);
        assert_eq!((got.added, got.removed), (2, 2));
        assert_eq!(got.hunks.len(), 2);
        assert_eq!(
            got.hunks[0].text,
            "@@ -1,5 +1,5 @@\n 1\n 2\n-3\n+three\n 4\n 5"
        );
        assert_eq!(
            (got.hunks[0].added, got.hunks[0].removed),
            (Some((3, 3)), Some((3, 3)))
        );
        assert_eq!(
            got.hunks[1].text,
            "@@ -15,6 +15,6 @@\n 15\n 16\n-17\n 18\n+18b\n 19\n 20"
        );
        assert_eq!(
            (got.hunks[1].added, got.hunks[1].removed),
            (Some((18, 18)), Some((17, 17)))
        );

        let near = lines(|lines| {
            lines[2] = "x".into();
            lines[7] = "y".into();
        });
        let near = diff(&old, &near, 2);
        assert_eq!(
            near.hunks.len(),
            1,
            "4 unchanged lines between: one hunk at --unified 2"
        );
        assert_eq!(near.hunks[0].added, Some((3, 8)));
    }

    #[test]
    fn a_new_or_deleted_file_is_one_hunk_of_its_lines() {
        let added = diff("", "a\nb\n", 3);
        assert_eq!(added.hunks[0].text, "@@ -0,0 +1,2 @@\n+a\n+b");
        assert_eq!(
            (added.hunks[0].added, added.hunks[0].removed),
            (Some((1, 2)), None)
        );
        let deleted = diff("a\n", "", 3);
        assert_eq!(deleted.hunks[0].text, "@@ -1 +0,0 @@\n-a");
        assert!(diff("same\n", "same\n", 3).hunks.is_empty());
    }
}
