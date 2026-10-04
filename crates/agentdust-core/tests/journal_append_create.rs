mod journal_support;
mod scratch;

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use agentdust_core::journal::{FrameWriter, JournalError};
use journal_support::{copies_on_disk, count_of, journal, named, names_in, rotate_by_rename};
use scratch::TempDir;

struct RotateWhenCreated<'a> {
    dir: &'a Path,
    left: u32,
    next_stamp: u64,
    created: u32,
}

impl FrameWriter for RotateWhenCreated<'_> {
    fn write(&mut self, mut file: &File, frame: &[u8]) -> io::Result<usize> {
        file.write(frame)
    }

    fn created(&mut self, _path: &Path) {
        self.created += 1;
        if self.left > 0 {
            self.left -= 1;
            rotate_by_rename(self.dir, self.next_stamp);
            self.next_stamp += 1;
        }
    }
}

fn rotating(dir: &Path, left: u32) -> RotateWhenCreated<'_> {
    RotateWhenCreated {
        dir,
        left,
        next_stamp: 100,
        created: 0,
    }
}

#[test]
fn a_rotation_between_the_creation_and_the_reopen_is_retried_and_the_record_is_written_once() {
    let dir = TempDir::absent("create-rotated-once");
    let mut writer = rotating(&dir, 1);
    let appended = journal(&dir).append_with(&named("a"), &mut writer).unwrap();
    assert_eq!(appended.attempts, 1);
    assert_eq!(writer.created, 2);
    assert_eq!(copies_on_disk(&dir, "a"), 1);
    assert_eq!(count_of(&journal(&dir).read().unwrap().records, "a"), 1);
}

#[test]
fn a_file_that_a_rotation_takes_after_every_creation_ends_in_an_error_and_writes_nothing() {
    let dir = TempDir::absent("create-rotated-always");
    let mut writer = rotating(&dir, 100);
    match journal(&dir).append_with(&named("a"), &mut writer) {
        Err(JournalError::Io(err)) => assert_eq!(err.kind(), io::ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
    assert_eq!(writer.created, 3);
    assert_eq!(copies_on_disk(&dir, "a"), 0);
    assert!(!names_in(&dir).contains(&"journal.jsonl".to_owned()));
}

#[test]
fn an_existing_journal_is_never_reported_as_created() {
    let dir = TempDir::absent("create-existing");
    journal(&dir).append(&named("first")).unwrap();
    let mut writer = rotating(&dir, 0);
    journal(&dir).append_with(&named("second"), &mut writer).unwrap();
    assert_eq!(writer.created, 0);
}
