mod journal_support;
mod scratch;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentdust_core::journal::volume::{FixedVolume, FsFacts, MNT_LOCAL, VolumeProbe, classify};
use agentdust_core::journal::{Journal, JournalError};
use journal_support::{frame, named, names_in};
use scratch::TempDir;

struct Counting {
    asked: Mutex<Vec<PathBuf>>,
}

impl VolumeProbe for Counting {
    fn probe(&self, path: &Path) -> io::Result<FsFacts> {
        self.asked.lock().unwrap().push(path.to_path_buf());
        Ok(classify("apfs", MNT_LOCAL))
    }
}

struct Failing;

impl VolumeProbe for Failing {
    fn probe(&self, _path: &Path) -> io::Result<FsFacts> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
}

fn unsupported(result: Result<impl std::fmt::Debug, JournalError>) -> FsFacts {
    match result {
        Err(JournalError::UnsupportedFilesystem(facts)) => facts,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_volume_that_is_not_local_apfs_gets_no_record_and_the_error_names_it() {
    let matrix = [
        ("apfs", 0),
        ("hfs", MNT_LOCAL),
        ("devfs", MNT_LOCAL),
        ("nfs", 0),
        ("smbfs", 0),
        ("autofs", 0),
    ];
    for (name, flags) in matrix {
        let dir = TempDir::absent("volume-refused");
        let volume = FixedVolume::new(classify(name, flags));
        let facts = unsupported(Journal::with_volume(&dir, &volume).append(&named("a")));
        assert_eq!(facts, classify(name, flags), "{name}");
        assert!(!facts.supported, "{name}");
    }
}

#[test]
fn a_refused_volume_creates_nothing_not_even_the_directory() {
    let dir = TempDir::absent("volume-nothing-created");
    let volume = FixedVolume::new(classify("nfs", 0));
    unsupported(Journal::with_volume(&dir, &volume).append(&named("a")));
    assert!(!dir.exists());
}

#[test]
fn a_refused_volume_leaves_an_existing_directory_and_journal_as_they_are() {
    let dir = TempDir::absent("volume-existing-untouched");
    journal_support::journal(&dir).append(&named("kept")).unwrap();
    let before = fs::read(dir.join("journal.jsonl")).unwrap();
    let volume = FixedVolume::new(classify("smbfs", 0));
    unsupported(Journal::with_volume(&dir, &volume).append(&named("refused")));
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), before);
    assert_eq!(names_in(&dir), ["journal.jsonl"]);
}

#[test]
fn a_refused_volume_does_not_create_the_journal_in_an_empty_directory() {
    let dir = TempDir::private("volume-empty-dir");
    let volume = FixedVolume::new(classify("nfs", 0));
    unsupported(Journal::with_volume(&dir, &volume).append(&named("a")));
    assert!(names_in(&dir).is_empty());
}

#[test]
fn a_local_apfs_volume_is_written() {
    let dir = TempDir::absent("volume-accepted");
    Journal::with_volume(&dir, &FixedVolume::apfs_local())
        .append(&named("a"))
        .unwrap();
    assert_eq!(fs::read(dir.join("journal.jsonl")).unwrap(), frame(&named("a")));
}

#[test]
fn one_append_asks_the_probe_once_about_the_directory() {
    let dir = TempDir::private("volume-one-probe");
    let probe = Counting {
        asked: Mutex::new(Vec::new()),
    };
    Journal::with_volume(&dir, &probe).append(&named("a")).unwrap();
    assert_eq!(*probe.asked.lock().unwrap(), vec![dir.to_path_buf()]);
}

#[test]
fn a_directory_that_does_not_exist_is_judged_by_its_nearest_existing_ancestor() {
    let root = TempDir::private("volume-ancestor");
    let probe = Counting {
        asked: Mutex::new(Vec::new()),
    };
    let dir = root.join("data");
    Journal::with_volume(&dir, &probe).append(&named("a")).unwrap();
    assert_eq!(*probe.asked.lock().unwrap(), vec![root.to_path_buf()]);
}

#[test]
fn a_directory_below_a_missing_parent_is_judged_by_its_nearest_existing_ancestor_and_not_created() {
    let root = TempDir::private("volume-ancestor-missing-parent");
    let probe = Counting {
        asked: Mutex::new(Vec::new()),
    };
    let dir = root.join("a/b");
    let result = Journal::with_volume(&dir, &probe).append(&named("a"));
    assert!(matches!(&result, Err(JournalError::Io(err)) if err.kind() == io::ErrorKind::NotFound));
    assert_eq!(*probe.asked.lock().unwrap(), vec![root.to_path_buf()]);
    assert!(!root.join("a").exists());
}

#[test]
fn a_probe_that_fails_stops_the_append_before_anything_is_written() {
    let dir = TempDir::absent("volume-probe-fails");
    match Journal::with_volume(&dir, &Failing).append(&named("a")) {
        Err(JournalError::Io(err)) => assert_eq!(err.kind(), io::ErrorKind::PermissionDenied),
        other => panic!("{other:?}"),
    }
    assert!(!dir.exists());
}

#[test]
fn the_status_prints_the_name_and_both_flags() {
    let dir = TempDir::private("volume-status");
    let apfs = FixedVolume::apfs_local();
    let local = Journal::with_volume(&dir, &apfs);
    assert_eq!(local.status().unwrap().describe(), "apfs, local, supported");
    let nfs = FixedVolume::new(classify("nfs", 0));
    assert_eq!(
        Journal::with_volume(&dir, &nfs).status().unwrap().describe(),
        "nfs, not local, not supported"
    );
}

#[cfg(target_os = "macos")]
mod on_macos {
    use std::path::Path;

    use agentdust_core::journal::{self, JournalError};

    use super::{TempDir, named};

    #[test]
    fn the_system_probe_accepts_the_temporary_directory() {
        let dir = TempDir::absent("volume-system-ok");
        journal::append(&dir, &named("a")).unwrap();
        assert_eq!(journal::read(&dir).unwrap().records.len(), 1);
        let status = journal::status(&dir).unwrap();
        assert_eq!(status.describe(), "apfs, local, supported");
    }

    #[test]
    fn a_directory_under_dev_is_refused_and_nothing_is_attempted_there() {
        let dir = Path::new("/dev/agentdust-journal-volume-test");
        match journal::append(dir, &named("a")) {
            Err(JournalError::UnsupportedFilesystem(facts)) => {
                assert_eq!(facts.name, "devfs");
                assert!(!facts.supported);
            }
            other => panic!("{other:?}"),
        }
        assert!(!dir.exists());
    }
}
