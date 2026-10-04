mod scratch;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use agentdust_core::cwd::{canonical_cwd, cwd_key, normalize};
use agentdust_core::digest::{Domain, keyed_digest};
use agentdust_core::journal::CwdKey;
use agentdust_core::secret::Secret;
use scratch::private_dir;

fn secret() -> Secret {
    Secret::from_bytes(std::array::from_fn(|i| i as u8))
}

fn other_secret() -> Secret {
    Secret::from_bytes(std::array::from_fn(|i| 255 - i as u8))
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn real(path: &Path) -> String {
    text(&fs::canonicalize(path).unwrap()).to_owned()
}

struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        let root = private_dir(name);
        fs::create_dir_all(root.join("deep/dir/sub")).unwrap();
        symlink(root.join("deep/dir"), root.join("link")).unwrap();
        Self { root }
    }

    fn at(&self, tail: &str) -> String {
        format!("{}/{tail}", text(&self.root))
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn normalize_removes_dot_segments_repeated_separators_and_the_trailing_slash() {
    let cases = [
        ("/", "/"),
        ("//", "/"),
        ("/a", "/a"),
        ("/a/", "/a"),
        ("//a///b//", "/a/b"),
        ("/a/./b/.", "/a/b"),
        ("/./a", "/a"),
        ("/a/b/..", "/a"),
        ("/a/b/../c", "/a/c"),
        ("/a/b/../../..", "/"),
        ("/..", "/"),
        ("/../a", "/a"),
        ("/a/../../b", "/b"),
    ];
    for (input, expected) in cases {
        assert_eq!(normalize(input).as_deref(), Some(expected), "{input}");
    }
}

#[test]
fn normalize_keeps_names_that_only_look_like_dot_segments() {
    for name in ["...", "..a", "a..", ".hidden", "a.b", "..."] {
        let input = format!("/x/{name}/y");
        assert_eq!(normalize(&input).as_deref(), Some(input.as_str()), "{name}");
    }
}

#[test]
fn normalize_keeps_spaces_case_unicode_and_control_characters_in_names() {
    for input in [
        "/Users/dev/My Project",
        "/Users/Dev/project",
        "/Users/d\u{e9}v/\u{4e2d}\u{6587}",
        "/a/\u{202e}x",
        "/a/tab\there",
        "/a/new\nline",
    ] {
        assert_eq!(normalize(input).as_deref(), Some(input), "{input:?}");
    }
}

#[test]
fn normalize_refuses_what_is_not_an_absolute_path() {
    for input in [
        "", "a", "a/b", "./a", "../a", "~", "~/a", " /a", "\0", "/a\0b", "/a/\0",
    ] {
        assert_eq!(normalize(input), None, "{input:?}");
    }
}

#[test]
fn a_path_that_does_not_exist_is_returned_normalised() {
    assert_eq!(
        canonical_cwd("/agentdust-fixture//a/./b/../c/").as_deref(),
        Some("/agentdust-fixture/a/c")
    );
}

#[test]
fn canonical_cwd_refuses_what_is_not_an_absolute_path() {
    for input in ["", "a", "a/b", "./a", "../a", "\0", "/a\0b"] {
        assert_eq!(canonical_cwd(input), None, "{input:?}");
    }
}

#[test]
fn an_existing_directory_resolves_to_its_real_path() {
    let tree = Tree::new("cwd-real");
    let expected = real(&tree.root.join("deep/dir/sub"));
    assert_eq!(canonical_cwd(&tree.at("deep/dir/sub")).unwrap(), expected);
    assert_eq!(canonical_cwd(&tree.at("deep/dir/sub/")).unwrap(), expected);
    assert_eq!(canonical_cwd(&tree.at("deep//dir/./sub")).unwrap(), expected);
}

#[test]
fn a_symlink_resolves_to_the_directory_it_points_to() {
    let tree = Tree::new("cwd-symlink");
    let expected = real(&tree.root.join("deep/dir"));
    assert_eq!(canonical_cwd(&tree.at("link")).unwrap(), expected);
    assert_eq!(canonical_cwd(&tree.at("link/")).unwrap(), expected);
    assert_eq!(
        canonical_cwd(&tree.at("link/sub")).unwrap(),
        real(&tree.root.join("deep/dir/sub"))
    );
}

#[test]
fn the_symlink_spelling_and_the_real_spelling_give_the_same_result() {
    let tree = Tree::new("cwd-same");
    assert_eq!(
        canonical_cwd(&tree.at("link/sub")),
        canonical_cwd(&tree.at("deep/dir/sub"))
    );
}

#[test]
fn dot_dot_is_removed_before_symlinks_are_resolved() {
    let tree = Tree::new("cwd-dotdot");
    fs::create_dir(tree.root.join("x")).unwrap();
    fs::create_dir(tree.root.join("deep/x")).unwrap();
    assert_eq!(
        canonical_cwd(&tree.at("link/../x")).unwrap(),
        real(&tree.root.join("x"))
    );
}

#[test]
fn a_path_that_does_not_exist_is_not_resolved_through_its_parents() {
    let tree = Tree::new("cwd-missing");
    assert_eq!(
        canonical_cwd(&tree.at("link/missing")).unwrap(),
        tree.at("link/missing")
    );
}

#[test]
fn a_symlink_loop_falls_back_to_the_normalised_path() {
    let tree = Tree::new("cwd-loop");
    symlink(tree.root.join("two"), tree.root.join("one")).unwrap();
    symlink(tree.root.join("one"), tree.root.join("two")).unwrap();
    assert_eq!(canonical_cwd(&tree.at("one/")).unwrap(), tree.at("one"));
}

#[test]
fn canonical_cwd_is_idempotent_on_resolved_and_unresolved_paths() {
    let tree = Tree::new("cwd-idempotent");
    for input in [
        tree.at("link/sub"),
        tree.at("link/missing/../sub/"),
        tree.at("deep/dir"),
        "/".to_owned(),
        "/agentdust-fixture/./a/".to_owned(),
    ] {
        let once = canonical_cwd(&input).unwrap();
        assert_eq!(canonical_cwd(&once).as_deref(), Some(once.as_str()), "{input}");
    }
}

#[test]
fn the_key_is_the_cwd_digest_of_the_canonical_path() {
    let key = cwd_key(&secret(), "/agentdust-fixture/project").unwrap();
    assert_eq!(
        key.as_str(),
        "3657eb6997470b89d15500eb6c2095e0304efccae14ff2381d92f00155fa9fb7"
    );
    assert_eq!(
        key,
        CwdKey::try_from(keyed_digest(
            secret().as_bytes(),
            Domain::Cwd,
            b"/agentdust-fixture/project"
        ))
        .unwrap()
    );
}

#[test]
fn the_key_is_64_lowercase_hex_characters() {
    let key = cwd_key(&secret(), "/agentdust-fixture/project").unwrap();
    assert_eq!(key.as_str().len(), 64);
}

#[test]
fn every_spelling_of_one_directory_gives_one_key() {
    let keys: Vec<_> = [
        "/agentdust-fixture/b",
        "/agentdust-fixture/b/",
        "/agentdust-fixture//b",
        "/agentdust-fixture/./b",
        "/agentdust-fixture/c/../b",
    ]
    .iter()
    .map(|input| cwd_key(&secret(), input).unwrap())
    .collect();
    assert!(keys.windows(2).all(|pair| pair[0] == pair[1]), "{keys:?}");
}

#[test]
fn a_symlinked_directory_and_its_target_give_one_key() {
    let tree = Tree::new("cwd-key-symlink");
    assert_eq!(
        cwd_key(&secret(), &tree.at("link/sub")),
        cwd_key(&secret(), &tree.at("deep/dir/sub"))
    );
}

#[test]
fn different_directories_and_different_secrets_give_different_keys() {
    let first = cwd_key(&secret(), "/agentdust-fixture/a").unwrap();
    assert_ne!(first, cwd_key(&secret(), "/agentdust-fixture/b").unwrap());
    assert_ne!(first, cwd_key(&other_secret(), "/agentdust-fixture/a").unwrap());
}

#[test]
fn the_key_uses_the_cwd_domain_not_the_session_domain() {
    let key = cwd_key(&secret(), "/agentdust-fixture/a").unwrap();
    let session = keyed_digest(secret().as_bytes(), Domain::Session, b"/agentdust-fixture/a");
    assert_ne!(key.as_str(), session);
}

#[test]
fn no_key_is_made_for_what_is_not_an_absolute_path() {
    for input in ["", "a/b", "./a", "../a", "\0", "/a\0b"] {
        assert_eq!(cwd_key(&secret(), input), None, "{input:?}");
    }
}
