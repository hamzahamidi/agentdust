#![no_main]

use std::collections::BTreeSet;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use agentdust_core::journal::{self, ReadReport, Record};
use agentdust_fuzz::bounds::{FILE_SEPARATOR, MAX_FILES, assert_bounded, order_violation, retained_bytes};
use agentdust_fuzz::meter::{self, Meter};
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn data_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("agentdust-fuzz-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .expect("a private scratch directory");
        dir
    })
}

fn file_name(index: usize, count: usize) -> String {
    if index + 1 == count {
        "journal.jsonl".to_owned()
    } else {
        format!("journal.{}.jsonl", 1_000 + index)
    }
}

fn place(parts: &[&[u8]]) {
    for (index, part) in parts.iter().enumerate() {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(data_dir().join(file_name(index, parts.len())))
            .expect("a fresh journal file");
        file.write_all(part).expect("the scratch file takes the input");
    }
}

fn clear(count: usize) {
    for index in 0..count {
        fs::remove_file(data_dir().join(file_name(index, count))).expect("the file placed for this input");
    }
}

fuzz_target!(|data: &[u8]| {
    let parts: Vec<&[u8]> = data.splitn(MAX_FILES, |byte| *byte == FILE_SEPARATOR).collect();
    place(&parts);
    let (report, peak) = meter::measure(|| journal::read(data_dir()));
    clear(parts.len());
    let report = report.expect("a private directory of private files reads");

    let each: Vec<ReadReport> = parts
        .iter()
        .map(|part| journal::decode(*part).expect("a slice never fails to read"))
        .collect();
    let every_record: usize = each.iter().map(|part| part.records.len()).sum();
    let retained: usize = each.iter().map(retained_bytes).sum();
    assert_bounded("read", peak, data.len(), retained);

    assert_eq!(report.records.len() + report.duplicates_removed, every_record);
    assert_eq!(
        report.malformed_lines,
        each.iter().map(|part| part.malformed_lines).sum::<usize>()
    );
    assert_eq!(
        report.torn_frames,
        each.iter().map(|part| part.torn_frames).sum::<usize>()
    );
    assert_eq!(
        report.newer_version_lines,
        each.iter().map(|part| part.newer_version_lines).sum::<usize>()
    );
    assert_eq!(
        report.unknown_kind_lines,
        each.iter().map(|part| part.unknown_kind_lines).sum::<usize>()
    );
    assert_eq!(
        report.truncated_last_line,
        each.iter().any(|part| part.truncated_last_line)
    );
    assert_eq!(report.unsupported_version, report.newer_version_lines > 0);
    assert_eq!(report.unsafe_files, 0);

    let returned: BTreeSet<&Record> = report.records.iter().collect();
    let decoded: BTreeSet<&Record> = each.iter().flat_map(|part| part.records.iter()).collect();
    assert_eq!(returned.len(), report.records.len());
    assert_eq!(returned, decoded);
    assert_eq!(order_violation(&report.records), None);
});
