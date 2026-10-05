use std::path::Path;

use agentdust_core::class::Class;
use agentdust_core::classifier::Evidence;
use agentdust_core::cwd::CwdRelation;
use agentdust_core::finding::{HumanDisplay, LiveText, ModelFinding, format_age};

fn sample() -> ModelFinding {
    ModelFinding {
        item_id: "p4242-1a2b".to_owned(),
        class: Class::OwnedEnded,
        exe_base: Some("node".to_owned()),
        pid: 4242,
        age_secs: 7500,
        evidence: vec![Evidence::OwnedTag, Evidence::OwnedAgentGone],
        cwd_relation: CwdRelation::OtherRepo,
    }
}

fn args(parts: &[&str]) -> Vec<Vec<u8>> {
    parts.iter().map(|part| part.as_bytes().to_vec()).collect()
}

#[test]
fn ages_use_the_largest_two_units() {
    let cases = [
        (0, "0s"),
        (59, "59s"),
        (60, "1m"),
        (3599, "59m"),
        (3600, "1h 00m"),
        (7500, "2h 05m"),
        (86_399, "23h 59m"),
        (86_400, "1d 0h"),
        (90_000, "1d 1h"),
        (864_000 + 3600 * 5, "10d 5h"),
    ];
    for (secs, text) in cases {
        assert_eq!(format_age(secs), text, "{secs}");
    }
}

#[test]
fn the_prompt_line_has_a_fixed_template_and_only_model_fields() {
    assert_eq!(
        HumanDisplay::prompt(&sample()),
        "p4242-1a2b  owned-ended  pid 4242  node  age 2h 05m  cwd other_repo  evidence owned.tag, owned.agent_gone"
    );
}

#[test]
fn a_hidden_name_and_empty_evidence_have_fixed_text() {
    let mut found = sample();
    found.exe_base = None;
    found.evidence.clear();
    found.class = Class::Suspect;
    assert_eq!(
        HumanDisplay::prompt(&found),
        "p4242-1a2b  suspect  pid 4242  (unlisted)  age 2h 05m  cwd other_repo  evidence none"
    );
}

#[test]
fn the_terminal_block_adds_the_command_and_the_directory() {
    let live = LiveText::new(
        Some(&args(&["node", "server.js", "--port", "3000"])),
        Some(Path::new("/Users/dev/app")),
    );
    assert_eq!(
        HumanDisplay::terminal(&sample(), &live),
        "p4242-1a2b  owned-ended  pid 4242  node  age 2h 05m  cwd other_repo\n\
         \x20 evidence: owned.tag, owned.agent_gone\n\
         \x20 command: node server.js --port 3000\n\
         \x20 directory: /Users/dev/app"
    );
}

#[test]
fn unreadable_live_text_has_fixed_text() {
    let live = LiveText::new(None, None);
    let shown = HumanDisplay::terminal(&sample(), &live);
    assert!(
        shown.ends_with("  command: (unreadable)\n  directory: (unreadable)"),
        "{shown}"
    );
}

#[test]
fn the_command_is_redacted_and_cut_at_120_characters() {
    let long = "x".repeat(300);
    let live = LiveText::new(Some(&args(&["tool", "--token=hunter2", &long])), None);
    let command = live.command.unwrap();
    assert_eq!(command.chars().count(), 120);
    assert!(!command.contains("hunter2"));
    assert!(command.ends_with("..."));
}

#[test]
fn the_directory_keeps_its_tail_within_80_characters() {
    let long = format!("/Users/dev/{}/project", "d".repeat(200));
    let live = LiveText::new(None, Some(Path::new(&long)));
    let directory = live.directory.unwrap();
    assert_eq!(directory.chars().count(), 80);
    assert!(directory.ends_with("/project"));
}

#[test]
fn hostile_text_cannot_add_lines_or_escape_sequences_to_the_block() {
    let live = LiveText::new(
        Some(&args(&["echo", "\n  evidence: forged\n\x1b[2J\u{202e}"])),
        Some(Path::new("/tmp/\n  command: forged")),
    );
    let shown = HumanDisplay::terminal(&sample(), &live);
    assert_eq!(shown.lines().count(), 4, "{shown}");
    assert!(!shown.contains('\x1b'));
    assert!(!shown.contains('\u{202e}'));
    assert!(shown.contains("\\x1b[2J"));
    assert!(shown.contains("\\u{202e}"));
}

#[test]
fn the_prompt_never_shows_live_text() {
    let prompt = HumanDisplay::prompt(&sample());
    assert!(!prompt.contains("command"));
    assert!(!prompt.contains('\n'));
}
