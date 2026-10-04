mod scratch;

use std::fs;
use std::path::{Path, PathBuf};

use agentdust_core::cwd::{CwdRelation, RelationContext, relate, repo_root};
use scratch::private_dir;

struct World {
    root: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let root = fs::canonicalize({
            let dir = private_dir(name);
            fs::create_dir_all(&dir).unwrap();
            dir
        })
        .unwrap();
        for dir in [
            "repo_a/.git",
            "repo_a/src/deep",
            "repo_b/.git",
            "repo_b/lib",
            "plain/inner",
            "home/.git",
            "home/projects/app",
            "home/projects/app/.git",
            "home/notes",
            "scratch/session",
            "scratch/session/repo_c/.git",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::create_dir_all(root.join("worktree")).unwrap();
        fs::write(root.join("worktree/.git"), "gitdir: /elsewhere\n").unwrap();
        Self { root }
    }

    fn at(&self, tail: &str) -> PathBuf {
        self.root.join(tail)
    }

    fn context(&self) -> RelationContext {
        RelationContext {
            reference: Some(self.at("repo_a/src")),
            home: Some(self.at("home")),
            temp_roots: vec![self.at("scratch")],
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_directory_in_the_same_repository_as_the_reference_is_same_repo() {
    let world = World::new("rel-same");
    for dir in ["repo_a", "repo_a/src", "repo_a/src/deep"] {
        assert_eq!(
            relate(&world.at(dir), &world.context()),
            CwdRelation::SameRepo,
            "{dir}"
        );
    }
}

#[test]
fn a_directory_in_another_repository_is_other_repo() {
    let world = World::new("rel-other");
    for dir in ["repo_b", "repo_b/lib", "worktree"] {
        assert_eq!(
            relate(&world.at(dir), &world.context()),
            CwdRelation::OtherRepo,
            "{dir}"
        );
    }
}

#[test]
fn a_git_file_marks_a_repository_as_a_git_directory_does() {
    let world = World::new("rel-gitfile");
    assert_eq!(repo_root(&world.at("worktree"), None), Some(world.at("worktree")));
}

#[test]
fn the_home_directory_itself_is_home_even_when_it_holds_a_git_directory() {
    let world = World::new("rel-home");
    assert_eq!(relate(&world.at("home"), &world.context()), CwdRelation::Home);
}

#[test]
fn a_git_directory_in_the_home_directory_does_not_make_everything_below_it_a_repository() {
    let world = World::new("rel-dotfiles");
    assert_eq!(
        relate(&world.at("home/notes"), &world.context()),
        CwdRelation::Other
    );
    assert_eq!(repo_root(&world.at("home/notes"), Some(&world.at("home"))), None);
}

#[test]
fn a_repository_below_the_home_directory_is_still_a_repository() {
    let world = World::new("rel-below-home");
    assert_eq!(
        relate(&world.at("home/projects/app"), &world.context()),
        CwdRelation::OtherRepo
    );
}

#[test]
fn a_directory_under_a_temp_root_is_temp() {
    let world = World::new("rel-temp");
    assert_eq!(relate(&world.at("scratch"), &world.context()), CwdRelation::Temp);
    assert_eq!(
        relate(&world.at("scratch/session"), &world.context()),
        CwdRelation::Temp
    );
}

#[test]
fn a_repository_wins_over_the_temp_root_that_holds_it() {
    let world = World::new("rel-temp-repo");
    assert_eq!(
        relate(&world.at("scratch/session/repo_c"), &world.context()),
        CwdRelation::OtherRepo
    );
}

#[test]
fn anything_else_is_other() {
    let world = World::new("rel-else");
    assert_eq!(relate(&world.at("plain"), &world.context()), CwdRelation::Other);
    assert_eq!(
        relate(&world.at("plain/inner"), &world.context()),
        CwdRelation::Other
    );
    assert_eq!(
        relate(Path::new("/definitely/not/here"), &world.context()),
        CwdRelation::Other
    );
}

#[test]
fn a_prefix_that_is_not_a_whole_component_is_not_temp() {
    let world = World::new("rel-prefix");
    fs::create_dir_all(world.at("scratch2/x")).unwrap();
    assert_eq!(
        relate(&world.at("scratch2/x"), &world.context()),
        CwdRelation::Other
    );
}

#[test]
fn without_a_reference_every_repository_is_another_repository() {
    let world = World::new("rel-noref");
    let context = RelationContext {
        reference: None,
        ..world.context()
    };
    assert_eq!(relate(&world.at("repo_a"), &context), CwdRelation::OtherRepo);
}

#[test]
fn a_reference_outside_every_repository_never_matches() {
    let world = World::new("rel-outside");
    let context = RelationContext {
        reference: Some(world.at("plain")),
        ..world.context()
    };
    assert_eq!(relate(&world.at("repo_a"), &context), CwdRelation::OtherRepo);
}

#[test]
fn the_search_for_a_repository_is_bounded() {
    let world = World::new("rel-deep");
    let mut deep = world.at("plain");
    for _ in 0..200 {
        deep.push("d");
    }
    assert_eq!(repo_root(&deep, None), None);
}

#[test]
fn the_system_context_knows_the_temp_directories_of_macos() {
    let context = RelationContext::system();
    for root in ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"] {
        assert!(
            context.temp_roots.iter().any(|path| path == Path::new(root)),
            "{root}"
        );
    }
    assert!(context.reference.is_some());
}
