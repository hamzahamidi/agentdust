mod ancestry_support;
mod common;

use agentdust_core::ancestry::{Anchor, MAX_DEPTH, find_agent};
use agentdust_core::identity::ProcessIdentity;
use ancestry_support::{Tree, chain};
use common::{changed, identity};

const CLAUDE: &str = "/Applications/Claude Code/claude.app/Contents/MacOS/claude";

fn at(pid: i32, exe_path: &str) -> ProcessIdentity {
    changed(&identity(pid), |id| id.evidence.exe_path = exe_path.into())
}

#[test]
fn the_parent_itself_is_the_agent_when_it_is_named_claude() {
    let tree = Tree::new().process(500, 400, CLAUDE).process(400, 1, "/bin/zsh");
    let found = find_agent(&tree, 500).unwrap();
    assert_eq!(found.identity, at(500, CLAUDE));
    assert_eq!(found.via, Anchor::Executable);
}

#[test]
fn the_walk_passes_shells_to_reach_the_agent() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, "/bin/zsh")
        .process(400, 300, CLAUDE)
        .process(300, 1, "/Applications/Terminal.app/Contents/MacOS/Terminal");
    let found = find_agent(&tree, 600).unwrap();
    assert_eq!(found.identity.kernel.pid, 400);
    assert_eq!(found.identity, at(400, CLAUDE));
}

#[test]
fn the_nearest_agent_wins() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, "/usr/local/bin/claude")
        .process(400, 300, "/bin/zsh")
        .process(300, 200, CLAUDE)
        .process(200, 1, "/bin/zsh");
    assert_eq!(find_agent(&tree, 600).unwrap().identity.kernel.pid, 500);
}

#[test]
fn node_running_a_script_that_mentions_claude_is_the_agent() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, "/opt/homebrew/bin/node")
        .script(
            500,
            "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        )
        .process(400, 1, "/bin/zsh");
    let found = find_agent(&tree, 600).unwrap();
    assert_eq!(found.identity, at(500, "/opt/homebrew/bin/node"));
    assert_eq!(found.via, Anchor::NodeScript);
}

#[test]
fn node_running_another_script_is_not_the_agent() {
    let tree = Tree::new()
        .process(500, 400, "/opt/homebrew/bin/node")
        .script(500, "/Users/dev/app/server.js")
        .process(400, 300, "/bin/zsh")
        .process(300, 1, "/bin/zsh");
    assert!(find_agent(&tree, 500).is_none());
    assert_eq!(tree.script_reads(), [500]);
}

#[test]
fn the_walk_continues_past_a_node_that_is_not_the_agent() {
    let tree = Tree::new()
        .process(500, 400, "/opt/homebrew/bin/node")
        .script(500, "/Users/dev/app/server.js")
        .process(400, 1, CLAUDE);
    assert_eq!(find_agent(&tree, 500).unwrap().identity.kernel.pid, 400);
}

#[test]
fn node_without_a_script_argument_or_with_an_unreadable_one_is_not_the_agent() {
    let without = Tree::new().process(500, 1, "/opt/homebrew/bin/node");
    assert!(find_agent(&without, 500).is_none());
    let unreadable = Tree::new()
        .process(500, 1, "/opt/homebrew/bin/node")
        .script_error(500);
    assert!(find_agent(&unreadable, 500).is_none());
}

#[test]
fn the_script_check_is_case_sensitive_and_needs_the_word() {
    for argument in [
        "/x/Claude/cli.js",
        "/x/CLAUDE.js",
        "/x/clau-de/cli.js",
        "/x/cli.js",
    ] {
        let tree = Tree::new()
            .process(500, 1, "/opt/homebrew/bin/node")
            .script(500, argument);
        assert!(find_agent(&tree, 500).is_none(), "{argument}");
    }
    for argument in ["/x/claude/cli.js", "/x/my-claude-wrapper.js", "claude"] {
        let tree = Tree::new()
            .process(500, 1, "/opt/homebrew/bin/node")
            .script(500, argument);
        assert!(find_agent(&tree, 500).is_some(), "{argument}");
    }
}

#[test]
fn only_an_executable_named_exactly_claude_or_node_is_considered() {
    for exe in [
        "/Applications/Claude.app/Contents/MacOS/Claude",
        "/usr/local/bin/claude-code",
        "/usr/local/bin/claudex",
        "/usr/local/bin/Claude",
        "/usr/local/bin/claude.sh",
        "/usr/local/claude/bin/sh",
        "/usr/local/bin/nodejs",
        "/usr/local/bin/node-claude",
    ] {
        let tree = Tree::new().process(500, 1, exe).script(500, "/x/claude/cli.js");
        assert!(find_agent(&tree, 500).is_none(), "{exe}");
    }
}

#[test]
fn a_path_with_a_trailing_slash_or_a_directory_called_claude_is_not_an_executable_named_claude() {
    for exe in ["/opt/claude/", "/opt/claude/bin/", "/claude/helper"] {
        let tree = Tree::new().process(500, 1, exe);
        assert!(find_agent(&tree, 500).is_none(), "{exe}");
    }
}

#[test]
fn the_script_argument_is_read_only_for_node() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, CLAUDE)
        .script(500, "/x/claude/cli.js")
        .process(400, 1, "/bin/zsh");
    assert!(find_agent(&tree, 600).is_some());
    assert_eq!(tree.script_reads(), Vec::<i32>::new());
}

#[test]
fn no_agent_on_the_way_to_launchd_gives_none() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, "/bin/zsh")
        .process(400, 1, "/usr/bin/login");
    assert!(find_agent(&tree, 600).is_none());
}

#[test]
fn a_start_at_launchd_or_below_examines_nothing() {
    let tree = Tree::new().process(1, 0, CLAUDE);
    for start in [1, 0, -1, i32::MIN] {
        assert!(find_agent(&tree, start).is_none(), "{start}");
    }
    assert_eq!(tree.all_reads(), Vec::<i32>::new());
}

#[test]
fn launchd_is_never_the_agent_even_when_it_looks_like_one() {
    let tree = Tree::new().process(500, 1, "/bin/sh").process(1, 0, CLAUDE);
    assert!(find_agent(&tree, 500).is_none());
    assert_eq!(tree.reads_of(1), 0);
}

#[test]
fn a_cycle_ends_the_walk_and_reads_each_process_once() {
    let tree = Tree::new().process(10, 11, "/bin/sh").process(11, 10, "/bin/zsh");
    assert!(find_agent(&tree, 10).is_none());
    assert_eq!(tree.reads_of(10), 1);
    assert_eq!(tree.reads_of(11), 1);
}

#[test]
fn a_process_that_is_its_own_parent_ends_the_walk() {
    let tree = Tree::new().process(10, 10, "/bin/sh");
    assert!(find_agent(&tree, 10).is_none());
    assert_eq!(tree.reads_of(10), 1);
}

#[test]
fn the_walk_examines_at_most_the_depth_limit() {
    assert_eq!(MAX_DEPTH, 64);
    let just_inside = chain(MAX_DEPTH + 10, Some(MAX_DEPTH - 1));
    assert_eq!(
        find_agent(&just_inside, 1000).unwrap().identity.kernel.pid,
        1000 + MAX_DEPTH as i32 - 1
    );
    let just_outside = chain(MAX_DEPTH + 10, Some(MAX_DEPTH));
    assert!(find_agent(&just_outside, 1000).is_none());
    assert_eq!(just_outside.all_reads().len(), MAX_DEPTH);
}

#[test]
fn an_ancestor_that_vanished_breaks_the_chain() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .gone(500, 400)
        .process(400, 1, CLAUDE);
    assert!(find_agent(&tree, 600).is_none());
}

#[test]
fn an_ancestor_whose_path_cannot_be_read_is_passed() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .unreadable_path(500, 400)
        .process(400, 1, CLAUDE);
    assert_eq!(find_agent(&tree, 600).unwrap().identity.kernel.pid, 400);
}

#[test]
fn an_ancestor_that_cannot_be_read_ends_the_walk() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .read_error(500, 400)
        .process(400, 1, CLAUDE);
    assert!(find_agent(&tree, 600).is_none());
}

#[test]
fn a_parent_that_cannot_be_read_ends_the_walk() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .parent_error(600)
        .process(500, 1, CLAUDE);
    assert!(find_agent(&tree, 600).is_none());
}

#[test]
fn a_process_replaced_between_its_read_and_its_parent_link_ends_the_walk() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .reused_before_the_parent_is_read(600)
        .process(500, 1, CLAUDE);
    assert!(find_agent(&tree, 600).is_none());
}

#[test]
fn an_agent_is_returned_without_asking_for_its_own_parent() {
    let tree = Tree::new().process(500, 400, CLAUDE).parent_error(500);
    assert_eq!(find_agent(&tree, 500).unwrap().identity.kernel.pid, 500);
}

#[test]
fn nothing_after_the_agent_is_read() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, CLAUDE)
        .process(400, 300, "/bin/zsh")
        .process(300, 1, "/bin/zsh");
    assert!(find_agent(&tree, 600).is_some());
    assert_eq!(tree.all_reads(), [600, 500]);
}

#[test]
fn an_unknown_start_pid_gives_none() {
    assert!(find_agent(&Tree::new(), 4242).is_none());
}

#[test]
fn codex_ancestry_does_not_anchor_a_claude_process() {
    use agentdust_core::ancestry::find_agent_for;
    use agentdust_core::journal::Agent;
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, CLAUDE)
        .process(400, 1, "/Applications/Codex.app/Contents/Resources/codex");
    assert_eq!(
        find_agent_for(&tree, 600, Agent::Codex)
            .unwrap()
            .identity
            .kernel
            .pid,
        400
    );
    assert_eq!(find_agent(&tree, 600).unwrap().identity.kernel.pid, 500);
    let unknown = Tree::new().process(600, 1, "/Applications/Codex.app/Contents/MacOS/Codex");
    assert!(find_agent_for(&unknown, 600, Agent::Codex).is_none());
    for exe in ["codex", "codex-aarch64-apple-darwin", "codex-x86_64-apple-darwin"] {
        let tree = Tree::new().process(600, 1, &format!("/usr/bin/{exe}"));
        assert!(find_agent_for(&tree, 600, Agent::Codex).is_some());
    }
}
