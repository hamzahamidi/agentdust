use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agentdust_core::journal::{AGENT_IDENTITY_KEYS, Class, RECORD_KEYS, scan};
use agentdust_core::secret::SECRET_FILE;

const MAX_DEPTH: usize = 3;
const MAX_READ: u64 = 32 * 1024 * 1024;

pub struct Sentinels {
    pub session_id: String,
    pub command: String,
    pub output_start: String,
    pub output_middle: String,
    pub output_end: String,
    pub user: String,
    pub repo: String,
    pub cwd: String,
    pub transcript: String,
    pub unknown_name: String,
    pub unknown_value: String,
    pub env_value: String,
    pub tag: String,
    pub process_dir: String,
    pub generated: Vec<String>,
}

impl Sentinels {
    pub fn new(run: &str) -> Self {
        let make = |base: &str| format!("{base}-{run}");
        let user = make("user-sentinel-wN8d");
        let repo = make("repo-sentinel-hJ5t");
        Self {
            session_id: make("session-sentinel-aT4n"),
            command: make("command-sentinel-vQ4x"),
            output_start: make("output-start-sentinel-pL2m"),
            output_middle: make("output-middle-sentinel-pL2m"),
            output_end: make("output-end-sentinel-pL2m"),
            cwd: format!("/Users/{user}/work/{repo}"),
            user,
            repo,
            transcript: make("transcript-sentinel-bF3y"),
            unknown_name: make("unknown-name-sentinel-rT6c"),
            unknown_value: make("unknown-value-sentinel-rT6c"),
            env_value: make("env-sentinel-dK9s"),
            tag: make("tag-sentinel-mC1v"),
            process_dir: make("process-dir-sentinel-eY7u"),
            generated: Vec::new(),
        }
    }

    pub fn also_never_stored(&mut self, value: impl Into<String>) {
        self.generated.push(value.into());
    }

    pub fn never_stored(&self) -> Vec<&str> {
        let mut values: Vec<&str> = vec![
            &self.command,
            &self.output_start,
            &self.output_middle,
            &self.output_end,
            &self.user,
            &self.repo,
            &self.cwd,
            &self.transcript,
            &self.unknown_name,
            &self.unknown_value,
            &self.env_value,
            &self.tag,
            &self.process_dir,
        ];
        values.extend(self.generated.iter().map(String::as_str));
        values
    }

    pub fn nowhere_outside_the_journal(&self) -> Vec<&str> {
        let mut values = self.never_stored();
        values.push(&self.session_id);
        values
    }
}

pub fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

pub fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let kind = fs::symlink_metadata(&path).unwrap().file_type();
        assert!(kind.is_file(), "{} is not a regular file", path.display());
        files.push(path);
    }
    files.sort();
    files
}

pub fn is_journal_file(name: &str) -> bool {
    if name == "journal.jsonl" {
        return true;
    }
    name.strip_prefix("journal.")
        .and_then(|rest| rest.strip_suffix(".jsonl"))
        .is_some_and(|stamp| !stamp.is_empty() && stamp.bytes().all(|byte| byte.is_ascii_digit()))
}

pub fn is_known_file(name: &str) -> bool {
    name == SECRET_FILE || name == "journal.maint" || is_journal_file(name)
}

pub struct Scan {
    pub names: BTreeSet<String>,
    pub keys: BTreeSet<String>,
    pub identity_keys: BTreeSet<String>,
    pub journal_files: usize,
}

pub fn scan_data_dir(dir: &Path, stage: &str, sentinels: &Sentinels) -> Scan {
    let mode = fs::metadata(dir).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o700, "{stage}: data directory mode");
    let mut found = Scan {
        names: BTreeSet::new(),
        keys: BTreeSet::new(),
        identity_keys: BTreeSet::new(),
        journal_files: 0,
    };
    let allowed: BTreeSet<&str> = RECORD_KEYS.iter().copied().collect();
    let allowed_identity: BTreeSet<&str> = AGENT_IDENTITY_KEYS.iter().copied().collect();
    for file in all_files(dir) {
        let name = file.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(is_known_file(&name), "{stage}: unexpected file {name}");
        let meta = fs::symlink_metadata(&file).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o600, "{stage}: {name} mode");
        assert_eq!(meta.nlink(), 1, "{stage}: {name} links");
        let bytes = fs::read(&file).unwrap();
        for needle in sentinels.never_stored() {
            assert!(!contains(&bytes, needle), "{stage}: {needle} found in {name}");
        }
        if is_journal_file(&name) {
            found.journal_files += 1;
            scan(&bytes[..], |raw, class| {
                assert!(
                    matches!(class, Class::Record(_)),
                    "{stage}: {name} holds a segment that is not a record: {class:?}"
                );
                let value: serde_json::Value = serde_json::from_slice(raw).unwrap();
                let keys: BTreeSet<String> = value.as_object().unwrap().keys().cloned().collect();
                let unlisted: Vec<&String> = keys
                    .iter()
                    .filter(|key| !allowed.contains(key.as_str()))
                    .collect();
                assert!(
                    unlisted.is_empty(),
                    "{stage}: keys outside the list in {name}: {unlisted:?}"
                );
                if let Some(identity) = value.get("agent_identity") {
                    let nested: BTreeSet<String> = identity
                        .as_object()
                        .unwrap_or_else(|| panic!("{stage}: agent_identity in {name} is not an object"))
                        .keys()
                        .cloned()
                        .collect();
                    let unlisted: Vec<&String> = nested
                        .iter()
                        .filter(|key| !allowed_identity.contains(key.as_str()))
                        .collect();
                    assert!(
                        unlisted.is_empty(),
                        "{stage}: identity keys outside the list in {name}: {unlisted:?}"
                    );
                    found.identity_keys.extend(nested);
                }
                found.keys.extend(keys);
            })
            .unwrap();
        } else {
            assert!(
                !contains(&bytes, &sentinels.session_id),
                "{stage}: the session id found in {name}"
            );
        }
        found.names.insert(name);
    }
    found
}

pub fn journal_bytes(dir: &Path) -> Vec<u8> {
    all_files(dir)
        .into_iter()
        .filter(|file| is_journal_file(file.file_name().unwrap().to_str().unwrap()))
        .flat_map(|file| fs::read(file).unwrap())
        .collect()
}

pub fn system_temp_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut real: Vec<PathBuf> = Vec::new();
    for candidate in [
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
    ] {
        let Ok(resolved) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !real.contains(&resolved) {
            real.push(resolved);
            roots.push(candidate);
        }
    }
    roots
}

pub struct TempWatch {
    since: (i64, i64),
    roots: Vec<PathBuf>,
    excluded: HashSet<(u64, u64)>,
}

impl TempWatch {
    pub fn start(excluded: &[&Path]) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Self::since(
            (now.as_secs() as i64, i64::from(now.subsec_nanos())),
            system_temp_roots(),
            excluded,
        )
    }

    pub fn since(since: (i64, i64), roots: Vec<PathBuf>, excluded: &[&Path]) -> Self {
        let excluded = excluded
            .iter()
            .map(|path| {
                let meta = fs::metadata(path).unwrap();
                (meta.dev(), meta.ino())
            })
            .collect();
        Self {
            since,
            roots,
            excluded,
        }
    }

    pub fn touched(&self) -> Vec<(PathBuf, bool)> {
        let mut touched = Vec::new();
        for root in &self.roots {
            self.walk(root, 1, &mut touched);
        }
        touched
    }

    fn walk(&self, dir: &Path, depth: usize, touched: &mut Vec<(PathBuf, bool)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if self.excluded.contains(&(meta.dev(), meta.ino())) {
                continue;
            }
            if (meta.ctime(), meta.ctime_nsec()) >= self.since {
                touched.push((path.clone(), meta.file_type().is_file()));
            }
            if meta.file_type().is_dir() && depth < MAX_DEPTH {
                self.walk(&path, depth + 1, touched);
            }
        }
    }

    pub fn leaks(&self, needles: &[&str]) -> Vec<String> {
        let mut found = Vec::new();
        for (path, regular) in self.touched() {
            let name = path.to_string_lossy();
            for needle in needles {
                if name.contains(needle) {
                    found.push(format!("{needle} in the name of {}", path.display()));
                }
            }
            if !regular {
                continue;
            }
            let Some(bytes) = read_capped(&path) else {
                continue;
            };
            for needle in needles {
                if contains(&bytes, needle) {
                    found.push(format!("{needle} in the bytes of {}", path.display()));
                }
            }
        }
        found
    }
}

fn read_capped(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.file_type().is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_READ).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}
