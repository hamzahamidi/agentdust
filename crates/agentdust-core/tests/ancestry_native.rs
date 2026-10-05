mod ancestry_support;
mod common;

use std::path::Path;

use agentdust_core::ancestry::{Anchor, agent_exe, find_agent};
use ancestry_support::Tree;

const NATIVE: &str = "/Users/dev/.local/share/claude/versions/2.1.289";

#[test]
fn the_native_installer_layout_is_an_agent_executable() {
    assert!(agent_exe(Path::new(NATIVE)));
    assert!(agent_exe(Path::new("/opt/claude/versions/9")));
}

#[test]
fn a_claude_basename_is_an_agent_executable() {
    assert!(agent_exe(Path::new("/usr/local/bin/claude")));
    assert!(agent_exe(Path::new("claude")));
}

#[test]
fn near_misses_are_not_agent_executables() {
    for path in [
        "/Users/dev/.local/share/Claude/versions/2.1.289",
        "/Users/dev/.local/share/claude/Versions/2.1.289",
        "/Users/dev/.local/share/claude/versions/2.1.289/bin/tool",
        "/Users/dev/.local/share/claude/versions/",
        "/Users/dev/.local/share/claude/other/2.1.289",
        "/Users/dev/.local/share/claudex/versions/2.1.289",
        "/Users/dev/.local/share/my-claude/versions/2.1.289",
        "/versions/2.1.289",
        "/usr/local/bin/claude-code",
        "/usr/local/bin/Claude",
        "/usr/local/bin/claude/",
        "/opt/homebrew/bin/node",
        "",
    ] {
        assert!(!agent_exe(Path::new(path)), "{path:?}");
    }
}

#[test]
fn the_walk_finds_a_native_installer_agent_above_the_hook() {
    let tree = Tree::new()
        .process(600, 500, "/bin/sh")
        .process(500, 400, NATIVE)
        .process(400, 1, "/bin/zsh");
    let found = find_agent(&tree, 600).unwrap();
    assert_eq!(found.identity.kernel.pid, 500);
    assert_eq!(found.via, Anchor::Executable);
}

#[test]
fn the_walk_does_not_stop_at_a_lookalike_layout() {
    let tree = Tree::new()
        .process(500, 400, "/Users/dev/.local/share/claudex/versions/2.1.289")
        .process(400, 1, "/bin/zsh");
    assert!(find_agent(&tree, 500).is_none());
}
