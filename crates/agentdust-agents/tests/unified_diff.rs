use agentdust_agents::diff::unified_diff;
use proptest::prelude::*;

fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

#[test]
fn identical_texts_have_no_diff() {
    assert_eq!(unified_diff("a\nb\n", "a\nb\n", "x", "y"), "");
    assert_eq!(unified_diff("", "", "x", "y"), "");
}

#[test]
fn a_change_in_the_middle_shows_three_lines_of_context() {
    let old = numbered(10);
    let new = old.replace("line 5\n", "line five\n");
    assert_eq!(
        unified_diff(&old, &new, "a/f", "b/f"),
        "--- a/f\n+++ b/f\n@@ -2,7 +2,7 @@\n line 2\n line 3\n line 4\n-line 5\n+line five\n line 6\n line 7\n line 8\n"
    );
}

#[test]
fn lines_added_at_the_end_show_the_trailing_context() {
    let old = "a\nb\nc\nd\n";
    let new = "a\nb\nc\nd\ne\nf\n";
    assert_eq!(
        unified_diff(old, new, "x", "y"),
        "--- x\n+++ y\n@@ -2,3 +2,5 @@\n b\n c\n d\n+e\n+f\n"
    );
}

#[test]
fn a_new_file_is_all_additions() {
    assert_eq!(
        unified_diff("", "{\n  \"a\": 1\n}\n", "/dev/null", "/home/u/settings.json"),
        "--- /dev/null\n+++ /home/u/settings.json\n@@ -0,0 +1,3 @@\n+{\n+  \"a\": 1\n+}\n"
    );
}

#[test]
fn an_emptied_file_is_all_removals() {
    assert_eq!(
        unified_diff("a\nb\n", "", "x", "y"),
        "--- x\n+++ y\n@@ -1,2 +0,0 @@\n-a\n-b\n"
    );
}

#[test]
fn a_single_line_hunk_omits_the_count() {
    assert_eq!(
        unified_diff("a\n", "b\n", "x", "y"),
        "--- x\n+++ y\n@@ -1 +1 @@\n-a\n+b\n"
    );
}

#[test]
fn distant_changes_make_two_hunks_and_near_changes_make_one() {
    let old = numbered(30);
    let far = old
        .replace("line 3\n", "three\n")
        .replace("line 28\n", "twenty-eight\n");
    let diff = unified_diff(&old, &far, "x", "y");
    assert_eq!(diff.matches("\n@@ ").count(), 2, "{diff}");
    let near = old
        .replace("line 10\n", "ten\n")
        .replace("line 16\n", "sixteen\n");
    let diff = unified_diff(&old, &near, "x", "y");
    assert_eq!(diff.matches("\n@@ ").count(), 1, "{diff}");
}

#[test]
fn a_missing_final_newline_is_marked() {
    assert_eq!(
        unified_diff("a\nb", "a\nb\n", "x", "y"),
        "--- x\n+++ y\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"
    );
    assert_eq!(
        unified_diff("a\nb\n", "a\nb", "x", "y"),
        "--- x\n+++ y\n@@ -1,2 +1,2 @@\n a\n-b\n+b\n\\ No newline at end of file\n"
    );
}

#[test]
fn an_insertion_into_a_json_member_list_shows_the_comma_change() {
    let old = "{\n  \"model\": \"opus\"\n}\n";
    let new = "{\n  \"model\": \"opus\",\n  \"hooks\": {}\n}\n";
    assert_eq!(
        unified_diff(old, new, "x", "y"),
        "--- x\n+++ y\n@@ -1,3 +1,4 @@\n {\n-  \"model\": \"opus\"\n+  \"model\": \"opus\",\n+  \"hooks\": {}\n }\n"
    );
}

#[test]
fn a_huge_unrelated_middle_still_produces_a_correct_diff() {
    let old: String = (0..3000).map(|n| format!("old {n}\n")).collect();
    let new: String = (0..3000).map(|n| format!("new {n}\n")).collect();
    let diff = unified_diff(&old, &new, "x", "y");
    assert_eq!(apply(&old, &diff).unwrap(), new);
}

fn apply(old: &str, diff: &str) -> Result<String, String> {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let diff_lines: Vec<&str> = diff.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut taken = 0usize;
    let mut index = 0usize;
    while index < diff_lines.len() {
        let line = diff_lines[index];
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            index += 1;
            continue;
        }
        let header = line
            .strip_prefix("@@ -")
            .ok_or_else(|| format!("unexpected line {line:?}"))?;
        let old_range = header.split(' ').next().unwrap();
        let (start, count) = match old_range.split_once(',') {
            Some((start, count)) => (start.parse::<usize>().unwrap(), count.parse::<usize>().unwrap()),
            None => (old_range.parse::<usize>().unwrap(), 1),
        };
        let first = if count == 0 { start } else { start - 1 };
        while taken < first {
            out.push_str(old_lines[taken]);
            taken += 1;
        }
        index += 1;
        while index < diff_lines.len() && !diff_lines[index].starts_with("@@ ") {
            let body = diff_lines[index];
            let (marker, text) = body.split_at(1);
            let bare = text.strip_suffix('\n').ok_or("a diff line lacks its newline")?;
            let no_newline = diff_lines
                .get(index + 1)
                .is_some_and(|next| next.starts_with('\\'));
            let content = if no_newline {
                bare.to_owned()
            } else {
                format!("{bare}\n")
            };
            match marker {
                " " | "-" => {
                    if old_lines.get(taken) != Some(&content.as_str()) {
                        return Err(format!(
                            "old line {taken} is {:?}, the diff says {content:?}",
                            old_lines.get(taken)
                        ));
                    }
                    taken += 1;
                    if marker == " " {
                        out.push_str(&content);
                    }
                }
                "+" => out.push_str(&content),
                "\\" => {}
                other => return Err(format!("unknown marker {other:?}")),
            }
            index += 1;
        }
    }
    while taken < old_lines.len() {
        out.push_str(old_lines[taken]);
        taken += 1;
    }
    Ok(out)
}

fn document() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(prop::sample::select(vec!["a", "b", "", "cc"]), 0..12),
        any::<bool>(),
    )
        .prop_map(|(lines, trailing)| {
            let mut text = lines.join("\n");
            if trailing && !lines.is_empty() {
                text.push('\n');
            }
            text
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn applying_the_diff_to_the_old_text_gives_the_new_text(old in document(), new in document()) {
        let diff = unified_diff(&old, &new, "x", "y");
        if old == new {
            prop_assert_eq!(diff, "");
        } else {
            prop_assert_eq!(apply(&old, &diff).unwrap(), new);
        }
    }
}
