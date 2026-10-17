//! A line diff for text values (a ConfigMap's file, a procedure, a DAG's
//! source): Myers' algorithm, capped, printed as unified hunks.

/// Edits past which two texts are only said to differ: Myers keeps a trace
/// that grows with the square of the edit distance.
// ponytail: 1,000 edits (about 8 MB of trace); a linear-space Myers if
// whole rewrites of long texts must be diffed.
const MAX_EDITS: usize = 1000;

/// Lines of context around each change, as `diff -u` prints.
const CONTEXT: usize = 3;

/// The most hunk lines one change prints.
const MAX_LINES: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Edit {
    Same,
    Delete,
    Insert,
}

/// Unified hunks between `left` and `right`, matching lines by `a` and `b`
/// (each line as compared: whitespace, case or comments dropped, one per
/// line) but printing the lines as they are.
pub(super) fn unified(
    left: &[&str],
    right: &[&str],
    a: Vec<String>,
    b: Vec<String>,
) -> Vec<String> {
    let Some(edits) = script(&a, &b) else {
        return vec![format!(
            "texts differ: {} lines against {}, more than {MAX_EDITS} edits apart",
            left.len(),
            right.len()
        )];
    };
    hunks(&edits, left, right)
}

/// The edits turning `a` into `b`, shortest first; `None` past [`MAX_EDITS`].
fn script(a: &[String], b: &[String]) -> Option<Vec<Edit>> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (core_a, core_b) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut edits = vec![Edit::Same; prefix];
    edits.extend(myers(core_a, core_b)?);
    edits.extend(vec![Edit::Same; suffix]);
    Some(edits)
}

/// Myers' O((N+M)D) greedy diff, keeping each round's furthest reaches
/// (diagonals -d-1 to d+1) to walk back from the end.
fn myers(a: &[String], b: &[String]) -> Option<Vec<Edit>> {
    let (n, m) = (a.len() as i64, b.len() as i64);
    let max = (n + m) as usize;
    let limit = max.min(MAX_EDITS) as i64;
    let offset = limit + 1;
    let mut v = vec![0i64; (2 * limit + 3) as usize];
    let mut trace: Vec<Vec<i64>> = Vec::new();
    let mut found = None;
    for d in 0..=limit {
        trace.push(v[(offset - d - 1) as usize..=(offset + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let at = (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[at - 1] < v[at + 1]) {
                v[at + 1]
            } else {
                v[at - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at] = x;
            if x >= n && y >= m {
                found = Some(d);
                break;
            }
            k += 2;
        }
        if found.is_some() {
            break;
        }
    }
    let last = found?;
    // Walk back: trace[d] holds round d-1's reaches, diagonal k at k + d + 1.
    let mut edits = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=last).rev() {
        let reach = &trace[d as usize];
        let at = |k: i64| reach[(k + d + 1) as usize];
        let k = x - y;
        let previous = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let px = if d == 0 { 0 } else { at(previous) };
        let py = if d == 0 { 0 } else { px - previous };
        while x > px && y > py {
            edits.push(Edit::Same);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            edits.push(if x == px { Edit::Insert } else { Edit::Delete });
        }
        x = px;
        y = py;
    }
    edits.reverse();
    Some(edits)
}

/// `@@ -a,b +c,d @@` hunks with [`CONTEXT`] lines around each change.
fn hunks(edits: &[Edit], left: &[&str], right: &[&str]) -> Vec<String> {
    // Each edit with the line numbers it stands at.
    let mut at = Vec::with_capacity(edits.len());
    let (mut i, mut j) = (0, 0);
    for edit in edits {
        at.push((*edit, i, j));
        match edit {
            Edit::Same => {
                i += 1;
                j += 1;
            }
            Edit::Delete => i += 1,
            Edit::Insert => j += 1,
        }
    }
    let changed: Vec<usize> = (0..at.len()).filter(|&e| at[e].0 != Edit::Same).collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < changed.len() {
        let start = changed[index].saturating_sub(CONTEXT);
        let mut end = (changed[index] + CONTEXT + 1).min(at.len());
        while index + 1 < changed.len() && changed[index + 1] <= end + CONTEXT {
            index += 1;
            end = (changed[index] + CONTEXT + 1).min(at.len());
        }
        let span = &at[start..end];
        let count = |side: Edit| {
            span.iter()
                .filter(|(e, ..)| *e == Edit::Same || *e == side)
                .count()
        };
        let (_, i0, j0) = span[0];
        let (left_count, right_count) = (count(Edit::Delete), count(Edit::Insert));
        out.push(format!(
            "@@ -{},{left_count} +{},{right_count} @@",
            i0 + usize::from(left_count > 0),
            j0 + usize::from(right_count > 0)
        ));
        for (edit, i, j) in span {
            out.push(match edit {
                Edit::Same => format!(" {}", left[*i]),
                Edit::Delete => format!("-{}", left[*i]),
                Edit::Insert => format!("+{}", right[*j]),
            });
        }
        index += 1;
    }
    if out.len() > MAX_LINES {
        let more = out.len() - MAX_LINES;
        out.truncate(MAX_LINES);
        out.push(format!("… {more} more lines"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(left: &str, right: &str) -> Vec<String> {
        let (l, r): (Vec<&str>, Vec<&str>) = (left.lines().collect(), right.lines().collect());
        let key = |lines: &[&str]| lines.iter().map(|line| (*line).to_owned()).collect();
        unified(&l, &r, key(&l), key(&r))
    }

    #[test]
    fn one_changed_line_prints_one_hunk_with_three_lines_of_context() {
        let left = "a\nb\nc\nd\ne\nf\ng\nh";
        let right = "a\nb\nc\nd\nE\nf\ng\nh";
        assert_eq!(
            diff(left, right),
            [
                "@@ -2,7 +2,7 @@",
                " b",
                " c",
                " d",
                "-e",
                "+E",
                " f",
                " g",
                " h"
            ]
        );
    }

    #[test]
    fn insertions_deletions_and_far_apart_changes_make_separate_hunks() {
        let left: Vec<String> = (1..=20).map(|n| n.to_string()).collect();
        let mut right = left.clone();
        right.remove(1);
        right.insert(15, "new".to_owned());
        let got = diff(&left.join("\n"), &right.join("\n"));
        assert_eq!(got[0], "@@ -1,5 +1,4 @@");
        assert!(got.contains(&"-2".to_owned()) && got.contains(&"+new".to_owned()));
        assert_eq!(got.iter().filter(|line| line.starts_with("@@")).count(), 2);
        assert_eq!(diff("", "x"), ["@@ -0,0 +1,1 @@", "+x"]);
    }

    #[test]
    fn a_rewrite_past_the_cap_is_only_said_to_differ() {
        let left: Vec<String> = (0..800).map(|n| format!("a{n}")).collect();
        let right: Vec<String> = (0..900).map(|n| format!("b{n}")).collect();
        assert_eq!(
            diff(&left.join("\n"), &right.join("\n")),
            ["texts differ: 800 lines against 900, more than 1000 edits apart"]
        );
    }

    #[test]
    fn the_key_decides_which_lines_match_and_the_originals_print() {
        let (l, r) = (vec!["  SELECT 1", "x"], vec!["select 1", "y"]);
        let key = |lines: &[&str]| {
            lines
                .iter()
                .map(|line| line.trim().to_lowercase())
                .collect()
        };
        let got = unified(&l, &r, key(&l), key(&r));
        assert_eq!(got, ["@@ -1,2 +1,2 @@", "   SELECT 1", "-x", "+y"]);
    }
}
