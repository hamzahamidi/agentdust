mod scratch;

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::thread;

use agentdust_core::safe_open::SafeOpenError;
use agentdust_core::tag::{SessionTag, TAG_BYTES, append_export, append_export_as};
use scratch::{make_fifo, private_dir, returns_promptly};

fn tag(byte: u8) -> SessionTag {
    SessionTag::from_bytes([byte; TAG_BYTES])
}

fn line(byte: u8) -> String {
    tag(byte).export_line()
}

fn file_with(dir: &Path, name: &str, mode: u32, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn euid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

#[test]
fn the_line_is_appended_after_what_the_file_already_holds() {
    let dir = private_dir("tag-env-append");
    let path = file_with(&dir, "env.sh", 0o600, "export A=1\n");
    append_export(&path, &tag(1)).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("export A=1\n\n{}", line(1))
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_line_that_lacks_its_newline_is_not_joined_to_the_export() {
    let dir = private_dir("tag-env-no-newline");
    let path = file_with(&dir, "env.sh", 0o600, "export A=1");
    append_export(&path, &tag(2)).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines, ["export A=1", line(2).trim_end()]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_empty_file_gets_the_line_and_nothing_else() {
    let dir = private_dir("tag-env-empty");
    let path = file_with(&dir, "env.sh", 0o600, "");
    append_export(&path, &tag(3)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), line(3));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_file_is_created_private_with_the_line() {
    let dir = private_dir("tag-env-create");
    let path = dir.join("env.sh");
    append_export(&path, &tag(4)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), line(4));
    assert_eq!(mode_of(&path), 0o600);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_mode_of_a_file_the_host_made_is_not_changed_and_does_not_matter() {
    let dir = private_dir("tag-env-mode");
    for mode in [0o644, 0o666, 0o640] {
        let path = file_with(&dir, &format!("env-{mode:o}.sh"), mode, "");
        append_export(&path, &tag(5)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), line(5), "{mode:o}");
        assert_eq!(mode_of(&path), mode, "{mode:o}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_symlink_is_refused_and_its_target_is_left_alone() {
    let dir = private_dir("tag-env-symlink");
    let target = file_with(&dir, "target.sh", 0o600, "keep\n");
    let link = dir.join("env.sh");
    symlink(&target, &link).unwrap();
    assert!(matches!(
        append_export(&link, &tag(6)),
        Err(SafeOpenError::Symlink)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dangling_symlink_is_refused_and_nothing_is_created_behind_it() {
    let dir = private_dir("tag-env-dangling");
    let target = dir.join("not-yet.sh");
    let link = dir.join("env.sh");
    symlink(&target, &link).unwrap();
    assert!(matches!(
        append_export(&link, &tag(7)),
        Err(SafeOpenError::Symlink)
    ));
    assert!(!target.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_with_another_hard_link_is_refused_and_unchanged() {
    let dir = private_dir("tag-env-hardlink");
    let path = file_with(&dir, "env.sh", 0o600, "keep\n");
    fs::hard_link(&path, dir.join("other.sh")).unwrap();
    assert!(matches!(
        append_export(&path, &tag(8)),
        Err(SafeOpenError::HardLinked { links: 2 })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep\n");
    assert_eq!(fs::read_to_string(dir.join("other.sh")).unwrap(), "keep\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_is_refused() {
    let dir = private_dir("tag-env-dir");
    let inner = dir.join("env.sh");
    fs::create_dir(&inner).unwrap();
    assert!(matches!(
        append_export(&inner, &tag(9)),
        Err(SafeOpenError::NotRegular)
    ));
    assert!(fs::read_dir(&inner).unwrap().next().is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_is_refused_without_blocking() {
    let dir = private_dir("tag-env-fifo");
    let path = dir.join("env.sh");
    make_fifo(&path);
    let result = returns_promptly({
        let path = path.clone();
        move || append_export(&path, &tag(10)).is_err()
    });
    assert!(result);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fifo_with_a_reader_is_refused_and_receives_nothing() {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = private_dir("tag-env-fifo-reader");
    let path = dir.join("env.sh");
    make_fifo(&path);
    let mut reader = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .unwrap();
    let result = returns_promptly({
        let path = path.clone();
        move || append_export(&path, &tag(10))
    });
    assert!(matches!(result, Err(SafeOpenError::NotRegular)), "{result:?}");
    let mut seen = Vec::new();
    match reader.read_to_end(&mut seen) {
        Ok(_) => {}
        Err(err) => assert_eq!(err.kind(), ErrorKind::WouldBlock),
    }
    assert!(seen.is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_of_another_owner_is_refused_and_unchanged() {
    let dir = private_dir("tag-env-owner");
    let path = file_with(&dir, "env.sh", 0o600, "keep\n");
    let stranger = euid() + 1;
    assert!(matches!(
        append_export_as(&path, &tag(11), stranger),
        Err(SafeOpenError::ForeignOwner { .. })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep\n");
    assert!(append_export_as(&path, &tag(11), fs::metadata(&path).unwrap().uid()).is_ok());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_relative_path_is_refused_and_nothing_is_created() {
    let dir = private_dir("tag-env-relative");
    let before: Vec<_> = fs::read_dir(".")
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    for relative in ["env.sh", "./env.sh", "../env.sh", ""] {
        match append_export(Path::new(relative), &tag(12)) {
            Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::InvalidInput, "{relative:?}"),
            other => panic!("{relative:?}: {other:?}"),
        }
    }
    let after: Vec<_> = fs::read_dir(".")
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_directory_is_an_error_and_is_not_created() {
    let dir = private_dir("tag-env-missing-dir");
    let path = dir.join("absent").join("env.sh");
    match append_export(&path, &tag(13)) {
        Err(SafeOpenError::Io(err)) => assert_eq!(err.kind(), ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
    assert!(!dir.join("absent").exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_reached_through_a_symlink_is_followed() {
    let dir = private_dir("tag-env-dir-link");
    let real = dir.join("real");
    fs::create_dir(&real).unwrap();
    symlink(&real, dir.join("alias")).unwrap();
    append_export(&dir.join("alias").join("env.sh"), &tag(14)).unwrap();
    assert_eq!(fs::read_to_string(real.join("env.sh")).unwrap(), line(14));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn concurrent_appends_never_tear_a_line() {
    let dir = private_dir("tag-env-concurrent");
    let path = file_with(&dir, "env.sh", 0o600, "");
    let handles: Vec<_> = (0..4u8)
        .map(|worker| {
            let path = path.clone();
            thread::spawn(move || {
                for round in 0..50u8 {
                    append_export(&path, &tag(worker * 50 + round)).unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let text = fs::read_to_string(&path).unwrap();
    let exports: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(exports.len(), 200);
    for export in exports {
        assert!(export.starts_with("export AGENTDUST_SESSION="), "{export}");
        assert_eq!(export.len(), "export AGENTDUST_SESSION=".len() + 32, "{export}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_file_is_sourced_by_a_shell_as_the_variable() {
    let dir = private_dir("tag-env-sourced");
    let path = file_with(&dir, "env.sh", 0o600, "export OTHER=kept\n");
    append_export(&path, &tag(0x2f)).unwrap();
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            ". '{}' && printf '%s %s' \"$OTHER\" \"$AGENTDUST_SESSION\"",
            path.display()
        ))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("kept {}", "2f".repeat(TAG_BYTES))
    );
    fs::remove_dir_all(&dir).unwrap();
}
