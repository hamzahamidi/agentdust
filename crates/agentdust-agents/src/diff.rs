const CONTEXT: usize = 3;
const MAX_CELLS: usize = 4_000_000;
const NO_NEWLINE: &str = "\\ No newline at end of file\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Keep,
    Delete,
    Insert,
}

pub fn unified_diff(old: &str, new: &str, old_label: &str, new_label: &str) -> String {
    if old == new {
        return String::new();
    }
    let before: Vec<&str> = old.split_inclusive('\n').collect();
    let after: Vec<&str> = new.split_inclusive('\n').collect();
    let ops = operations(&before, &after);
    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    for (start, end) in hunk_ranges(&ops) {
        write_hunk(&mut out, &ops[..start], &ops[start..end]);
    }
    out
}

fn operations<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<(Op, &'a str)> {
    let prefix = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old_mid, new_mid) = (
        &before[prefix..before.len() - suffix],
        &after[prefix..after.len() - suffix],
    );
    let mut ops: Vec<(Op, &str)> = before[..prefix].iter().map(|line| (Op::Keep, *line)).collect();
    ops.extend(middle(old_mid, new_mid));
    ops.extend(
        before[before.len() - suffix..]
            .iter()
            .map(|line| (Op::Keep, *line)),
    );
    ops
}

fn middle<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(Op, &'a str)> {
    let (rows, columns) = (old.len() + 1, new.len() + 1);
    if rows.saturating_mul(columns) > MAX_CELLS {
        let mut ops: Vec<(Op, &str)> = old.iter().map(|line| (Op::Delete, *line)).collect();
        ops.extend(new.iter().map(|line| (Op::Insert, *line)));
        return ops;
    }
    let mut common = vec![0u32; rows * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i * columns + j] = if old[i] == new[j] {
                common[(i + 1) * columns + j + 1] + 1
            } else {
                common[(i + 1) * columns + j].max(common[i * columns + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::new();
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            ops.push((Op::Keep, old[i]));
            i += 1;
            j += 1;
        } else if common[(i + 1) * columns + j] >= common[i * columns + j + 1] {
            ops.push((Op::Delete, old[i]));
            i += 1;
        } else {
            ops.push((Op::Insert, new[j]));
            j += 1;
        }
    }
    ops.extend(old[i..].iter().map(|line| (Op::Delete, *line)));
    ops.extend(new[j..].iter().map(|line| (Op::Insert, *line)));
    ops
}

fn hunk_ranges(ops: &[(Op, &str)]) -> Vec<(usize, usize)> {
    let changes: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, (op, _))| *op != Op::Keep)
        .map(|(index, _)| index)
        .collect();
    let mut ranges = Vec::new();
    let mut group: Option<(usize, usize)> = None;
    for index in changes {
        group = match group {
            Some((first, last)) if index - last - 1 <= 2 * CONTEXT => Some((first, index)),
            Some((first, last)) => {
                ranges.push((first, last));
                Some((index, index))
            }
            None => Some((index, index)),
        };
    }
    ranges.extend(group);
    ranges
        .into_iter()
        .map(|(first, last)| (first.saturating_sub(CONTEXT), (last + 1 + CONTEXT).min(ops.len())))
        .collect()
}

fn write_hunk(out: &mut String, preceding: &[(Op, &str)], hunk: &[(Op, &str)]) {
    let old_before = preceding.iter().filter(|(op, _)| *op != Op::Insert).count();
    let new_before = preceding.iter().filter(|(op, _)| *op != Op::Delete).count();
    let old_count = hunk.iter().filter(|(op, _)| *op != Op::Insert).count();
    let new_count = hunk.iter().filter(|(op, _)| *op != Op::Delete).count();
    out.push_str(&format!(
        "@@ -{} +{} @@\n",
        range(old_before, old_count),
        range(new_before, new_count)
    ));
    for (op, line) in hunk {
        out.push(match op {
            Op::Keep => ' ',
            Op::Delete => '-',
            Op::Insert => '+',
        });
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
            out.push_str(NO_NEWLINE);
        }
    }
}

fn range(before: usize, count: usize) -> String {
    match count {
        0 => format!("{before},0"),
        1 => format!("{}", before + 1),
        _ => format!("{},{count}", before + 1),
    }
}
