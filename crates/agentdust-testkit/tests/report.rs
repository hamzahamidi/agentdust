use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agentdust_testkit::report::{self, Report, ReportError};
use agentdust_testkit::wait_until;

const HANG_GUARD: Duration = Duration::from_secs(60);
const MIN_ROUNDS: u32 = 6;
const OBSERVED: u32 = 20;

fn report(env: Option<&[u8]>) -> Report {
    Report {
        pid: 4242,
        ppid: 100,
        sid: 4242,
        env: env.map(<[u8]>::to_vec),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdust-report-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_report_without_a_variable_is_three_lines() {
    assert_eq!(report(None).render(), b"pid=4242\nppid=100\nsid=4242\n");
}

#[test]
fn a_report_with_a_variable_ends_with_its_raw_value() {
    assert_eq!(
        report(Some(b"tag-1")).render(),
        b"pid=4242\nppid=100\nsid=4242\nenv=tag-1"
    );
}

#[test]
fn a_variable_set_to_nothing_differs_from_an_unset_variable() {
    let empty = report(Some(b""));
    let unset = report(None);
    assert_ne!(empty.render(), unset.render());
    assert_eq!(Report::parse(&empty.render()).unwrap().env, Some(Vec::new()));
    assert_eq!(Report::parse(&unset.render()).unwrap().env, None);
}

#[test]
fn every_kind_of_variable_value_round_trips() {
    let values: [&[u8]; 6] = [
        b"plain",
        b"two\nlines\n",
        b"\xff\xfe not utf8",
        b"pid=9\nppid=9\nsid=9\n",
        b"env=nested",
        &[0u8, 1, 2, 10, 13],
    ];
    for value in values {
        let original = report(Some(value));
        assert_eq!(Report::parse(&original.render()).unwrap(), original, "{value:?}");
    }
}

#[test]
fn a_large_value_round_trips() {
    let value = vec![b'x'; 1 << 20];
    let original = report(Some(&value));
    assert_eq!(Report::parse(&original.render()).unwrap(), original);
}

#[test]
fn a_report_that_is_not_in_the_expected_shape_is_refused() {
    let cases: [&[u8]; 14] = [
        b"",
        b"pid=1\n",
        b"pid=1\nppid=1\n",
        b"pid=1\nppid=1\nsid=1",
        b"ppid=1\npid=1\nsid=1\n",
        b"pid=1\nsid=1\nppid=1\n",
        b"pid=1\nppid=1\nsid=1\ngarbage",
        b"pid=1\nppid=1\nsid=1\n\n",
        b"pid=1\nppid=1\nsid=1\nenv",
        b"PID=1\nppid=1\nsid=1\n",
        b"pid=1 \nppid=1\nsid=1\n",
        b"pid=\nppid=1\nsid=1\n",
        b"pid=1\r\nppid=1\r\nsid=1\r\n",
        b"\npid=1\nppid=1\nsid=1\n",
    ];
    for case in cases {
        assert!(
            Report::parse(case).is_err(),
            "{:?}",
            String::from_utf8_lossy(case)
        );
    }
}

#[test]
fn a_pid_or_session_that_could_address_a_group_or_every_process_is_refused() {
    for (pid, ppid, sid) in [
        (0, 1, 1),
        (-1, 1, 1),
        (1, 1, 0),
        (1, 1, -1),
        (1, -1, 1),
        (i32::MIN, 1, 1),
    ] {
        let text = format!("pid={pid}\nppid={ppid}\nsid={sid}\n");
        assert!(Report::parse(text.as_bytes()).is_err(), "{text:?}");
    }
    assert!(Report::parse(b"pid=1\nppid=0\nsid=1\n").is_ok());
}

#[test]
fn numbers_must_be_written_the_canonical_way() {
    for text in [
        "pid=+5\nppid=1\nsid=1\n",
        "pid=05\nppid=1\nsid=1\n",
        "pid=5\nppid=01\nsid=1\n",
        "pid=5\nppid=1\nsid=+1\n",
        "pid=2147483648\nppid=1\nsid=1\n",
        "pid=99999999999999999999\nppid=1\nsid=1\n",
        "pid=0x10\nppid=1\nsid=1\n",
    ] {
        assert!(Report::parse(text.as_bytes()).is_err(), "{text:?}");
    }
    assert_eq!(
        Report::parse(b"pid=2147483647\nppid=1\nsid=1\n").unwrap().pid,
        i32::MAX
    );
}

#[test]
fn a_written_report_is_read_back_and_leaves_no_partial_file() {
    let dir = scratch("write");
    let path = dir.join("one.report");
    let original = report(Some(b"value"));
    report::write(&path, &original).unwrap();
    assert_eq!(report::read(&path).unwrap(), Some(original));
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["one.report".to_owned()]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_partial_file_left_by_an_earlier_write_does_not_block_a_new_one() {
    let dir = scratch("stale-partial");
    let path = dir.join("one.report");
    fs::write(dir.join("one.report.partial"), b"left behind by a killed writer").unwrap();
    let original = report(None);
    report::write(&path, &original).unwrap();
    assert_eq!(report::read(&path).unwrap(), Some(original));
    assert!(!dir.join("one.report.partial").exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_report_cannot_be_written_into_a_directory_that_does_not_exist() {
    let dir = scratch("no-directory");
    let result = report::write(&dir.join("missing/one.report"), &report(None));
    assert!(matches!(result, Err(e) if e.kind() == std::io::ErrorKind::NotFound));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_report_that_does_not_exist_reads_as_none() {
    let dir = scratch("absent");
    assert_eq!(report::read(&dir.join("missing")).unwrap(), None);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_malformed_report_file_is_an_error_and_not_none() {
    let dir = scratch("malformed");
    let path = dir.join("bad");
    fs::write(&path, b"pid=1\n").unwrap();
    assert!(matches!(report::read(&path), Err(ReportError::Malformed(_))));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_report_is_never_visible_half_written() {
    let dir = scratch("race");
    let value = vec![b'v'; 4 << 20];
    let original = report(Some(&value));
    let stop = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicU32::new(0));
    let seen = Arc::new(AtomicU32::new(0));
    let defects = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut readers = Vec::new();
    for n in 0..2 {
        let path = dir.join(format!("r{n}"));
        let (stop, started, seen) = (Arc::clone(&stop), Arc::clone(&started), Arc::clone(&seen));
        let defects = Arc::clone(&defects);
        let expected = original.clone();
        readers.push(thread::spawn(move || {
            started.fetch_add(1, Ordering::SeqCst);
            while !stop.load(Ordering::SeqCst) {
                let defect = match report::read(&path) {
                    Ok(Some(found)) if found == expected => {
                        seen.fetch_add(1, Ordering::SeqCst);
                        continue;
                    }
                    Ok(Some(_)) => "a report with other content was visible".to_owned(),
                    Ok(None) => continue,
                    Err(e) => format!("a half written report was visible: {e}"),
                };
                defects.lock().unwrap().push(defect);
                stop.store(true, Ordering::SeqCst);
            }
        }));
    }
    assert!(wait_until(|| started.load(Ordering::SeqCst) == 2, HANG_GUARD));
    let deadline = Instant::now() + HANG_GUARD;
    let mut round = 0;
    while !stop.load(Ordering::SeqCst)
        && (round < MIN_ROUNDS || seen.load(Ordering::SeqCst) < OBSERVED)
        && Instant::now() < deadline
    {
        for n in 0..2 {
            let path = dir.join(format!("r{n}"));
            if round % 2 == 1 {
                fs::remove_file(&path).unwrap();
            }
            report::write(&path, &original).unwrap();
        }
        round += 1;
    }
    stop.store(true, Ordering::SeqCst);
    for reader in readers {
        reader.join().unwrap();
    }
    assert_eq!(*defects.lock().unwrap(), Vec::<String>::new());
    assert!(seen.load(Ordering::SeqCst) >= OBSERVED);
    fs::remove_dir_all(&dir).unwrap();
}
