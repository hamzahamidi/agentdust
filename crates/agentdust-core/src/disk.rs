use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

use serde::Serialize;

const MAX_ENTRIES: u64 = 100_000;
const MAX_DEPTH: usize = 64;
const MAX_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Rebuildable,
    History,
    Worktree,
    ApplicationState,
    Unknown,
}

#[derive(Serialize)]
pub struct Usage {
    pub category: Category,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub files: u64,
    pub directories: u64,
}

#[derive(Default, Serialize)]
pub struct Skipped {
    pub symlinks: u64,
    pub special_files: u64,
    pub other_filesystems: u64,
    pub duplicate_inodes: u64,
    pub errors: u64,
    pub depth_limit: u64,
}

#[derive(Serialize)]
pub struct RootReport {
    pub root: &'static str,
    pub status: &'static str,
    pub usage: Vec<Usage>,
    pub skipped: Skipped,
}

#[derive(Serialize)]
pub struct DiskReport {
    pub schema_version: u32,
    pub roots: Vec<RootReport>,
    pub entries_examined: u64,
    pub limit_reached: bool,
    pub read_only: bool,
}

struct Scanner {
    started: Instant,
    entries: u64,
    limited: bool,
    seen: HashSet<(u64, u64)>,
}

#[derive(Clone, Copy)]
struct WalkOptions {
    category: Category,
    depth: usize,
    classify: bool,
    projects: bool,
    cache: bool,
}

struct WalkEntry<'a> {
    device: u64,
    options: WalkOptions,
    name: &'a CStr,
    category: Category,
    metadata: &'a libc::stat,
}

impl WalkOptions {
    fn child_category(self, name: &[u8], regular: bool) -> Category {
        if self.classify && self.depth == 0 {
            classify_name(name, regular)
        } else if self.classify && self.depth == 1 && self.cache && name == b"changelog.md" && regular {
            Category::Rebuildable
        } else if self.projects && self.depth == 2 && name == b"memory" {
            Category::ApplicationState
        } else {
            self.category
        }
    }

    fn descend(self, name: &[u8], category: Category) -> Self {
        Self {
            category,
            depth: self.depth + 1,
            projects: self.projects || (self.classify && self.depth == 0 && name == b"projects"),
            cache: self.classify && self.depth == 0 && name == b"cache",
            ..self
        }
    }
}

#[cfg(target_os = "linux")]
fn device_id(device: libc::dev_t) -> u64 {
    device
}

#[cfg(not(target_os = "linux"))]
fn device_id(device: libc::dev_t) -> u64 {
    u64::try_from(device).unwrap_or_default()
}

pub fn report(config: &Path, project: &Path) -> DiskReport {
    let mut scanner = Scanner {
        started: Instant::now(),
        entries: 0,
        limited: false,
        seen: HashSet::new(),
    };
    let roots = vec![
        scanner.root("claude_config", config, Category::Unknown, true),
        scanner.root(
            "current_project_worktrees",
            &project.join(".claude/worktrees"),
            Category::Worktree,
            false,
        ),
    ];
    DiskReport {
        schema_version: 1,
        roots,
        entries_examined: scanner.entries,
        limit_reached: scanner.limited,
        read_only: true,
    }
}

impl Scanner {
    fn exhausted(&mut self) -> bool {
        if self.entries >= MAX_ENTRIES || self.started.elapsed() >= MAX_DURATION {
            self.limited = true;
        }
        self.limited
    }

    fn root(&mut self, label: &'static str, path: &Path, category: Category, classify: bool) -> RootReport {
        let mut result = RootReport {
            root: label,
            status: "complete",
            usage: [
                Category::Rebuildable,
                Category::History,
                Category::Worktree,
                Category::ApplicationState,
                Category::Unknown,
            ]
            .into_iter()
            .map(|category| Usage {
                category,
                logical_bytes: 0,
                allocated_bytes: 0,
                files: 0,
                directories: 0,
            })
            .collect(),
            skipped: Skipped::default(),
        };
        if self.exhausted() {
            result.status = "partial";
            return result;
        }
        match open_root(path) {
            Ok(file) => match file.metadata() {
                Ok(meta) => {
                    if !self.seen.insert((meta.dev(), meta.ino())) {
                        result.skipped.duplicate_inodes += 1;
                        result.status = "already_counted";
                        return result;
                    }
                    add(&mut result, category, &meta);
                    self.walk(
                        &file,
                        meta.dev(),
                        WalkOptions {
                            category,
                            depth: 0,
                            classify,
                            projects: false,
                            cache: false,
                        },
                        &mut result,
                    );
                    if self.limited
                        || result.skipped.errors > 0
                        || result.skipped.depth_limit > 0
                        || result.skipped.other_filesystems > 0
                    {
                        result.status = "partial";
                    }
                }
                Err(_) => {
                    result.skipped.errors += 1;
                    result.status = "partial";
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => result.status = "not_present",
            Err(_) => {
                result.skipped.errors += 1;
                result.status = "unavailable";
            }
        }
        result
    }

    fn walk(&mut self, file: &File, device: u64, options: WalkOptions, result: &mut RootReport) {
        if options.depth >= MAX_DEPTH {
            result.skipped.depth_limit += 1;
            return;
        }
        let directory = match Directory::new(file.as_raw_fd()) {
            Ok(directory) => directory,
            Err(_) => {
                result.skipped.errors += 1;
                return;
            }
        };
        loop {
            if self.exhausted() {
                return;
            }
            let name = match directory.next() {
                Ok(Some(name)) => name,
                Ok(None) => return,
                Err(_) => {
                    result.skipped.errors += 1;
                    return;
                }
            };
            if name.as_bytes() == b"." || name.as_bytes() == b".." {
                continue;
            }
            self.walk_entry(file, device, options, &name, result);
        }
    }

    fn walk_entry(
        &mut self,
        file: &File,
        device: u64,
        options: WalkOptions,
        name: &CStr,
        result: &mut RootReport,
    ) {
        self.entries += 1;
        let metadata = match stat_at(file.as_raw_fd(), name) {
            Ok(metadata) => metadata,
            Err(_) => {
                result.skipped.errors += 1;
                return;
            }
        };
        let kind = metadata.st_mode & libc::S_IFMT;
        if kind == libc::S_IFLNK {
            result.skipped.symlinks += 1;
            return;
        }
        if kind != libc::S_IFREG && kind != libc::S_IFDIR {
            result.skipped.special_files += 1;
            return;
        }
        if device_id(metadata.st_dev) != device {
            result.skipped.other_filesystems += 1;
            return;
        }
        if self.seen.contains(&(device_id(metadata.st_dev), metadata.st_ino)) {
            result.skipped.duplicate_inodes += 1;
            return;
        }
        let category = options.child_category(name.to_bytes(), kind == libc::S_IFREG);
        if kind == libc::S_IFDIR {
            self.walk_directory(
                file,
                WalkEntry {
                    device,
                    options,
                    name,
                    category,
                    metadata: &metadata,
                },
                result,
            );
        } else {
            self.add_file(category, &metadata, result);
        }
    }

    fn walk_directory(&mut self, file: &File, entry: WalkEntry<'_>, result: &mut RootReport) {
        let child = open_at(file.as_raw_fd(), entry.name).and_then(|child| {
            let actual = child.metadata()?;
            if actual.dev() != device_id(entry.metadata.st_dev) || actual.ino() != entry.metadata.st_ino {
                return Err(io::Error::other("directory changed"));
            }
            Ok((child, actual))
        });
        match child {
            Ok((child, actual)) => {
                self.seen.insert((actual.dev(), actual.ino()));
                add(result, entry.category, &actual);
                self.walk(
                    &child,
                    entry.device,
                    entry.options.descend(entry.name.to_bytes(), entry.category),
                    result,
                );
            }
            Err(_) => result.skipped.errors += 1,
        }
    }

    fn add_file(&mut self, category: Category, metadata: &libc::stat, result: &mut RootReport) {
        self.seen.insert((device_id(metadata.st_dev), metadata.st_ino));
        let usage = &mut result.usage[category as usize];
        usage.files += 1;
        usage.logical_bytes = usage.logical_bytes.saturating_add(metadata.st_size.max(0) as u64);
        usage.allocated_bytes = usage
            .allocated_bytes
            .saturating_add((metadata.st_blocks.max(0) as u64).saturating_mul(512));
    }
}

fn classify_name(name: &[u8], regular: bool) -> Category {
    match name {
        b"remote-settings.json" | b"policy-limits.json" | b"policy-limits.json.stamp.json" if regular => {
            Category::Rebuildable
        }
        b"projects" | b"file-history" | b"history.jsonl" | b"plans" | b"backups" | b"uploads"
        | b"paste-cache" | b"image-cache" | b"feedback-bundles" | b"feedback" | b"usage-data"
        | b"stats-cache.json" => Category::History,
        b"plugins"
        | b"skills"
        | b"commands"
        | b"agents"
        | b"rules"
        | b"agent-memory"
        | b"jobs"
        | b"daemon"
        | b"sessions"
        | b"session-env"
        | b"shell-snapshots"
        | b"tasks"
        | b"settings.json"
        | b"settings.local.json"
        | b".credentials.json"
        | b"CLAUDE.md"
        | b"keybindings.json"
        | b"themes"
        | b"output-styles"
        | b"debug"
        | b"logs"
        | b"todos"
        | b"statsig" => Category::ApplicationState,
        _ => Category::Unknown,
    }
}

fn add(result: &mut RootReport, category: Category, metadata: &std::fs::Metadata) {
    let usage = &mut result.usage[category as usize];
    usage.directories += 1;
    usage.allocated_bytes = usage
        .allocated_bytes
        .saturating_add(metadata.blocks().saturating_mul(512));
}

fn open_root(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::other("absolute root required"));
    }
    let mut file = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                let name = CString::new(name.as_bytes()).map_err(|_| io::Error::other("invalid root"))?;
                file = open_at(file.as_raw_fd(), &name)?;
            }
            _ => return Err(io::Error::other("invalid root")),
        }
    }
    Ok(file)
}

fn open_at(parent: RawFd, name: &CStr) -> io::Result<File> {
    // SAFETY: name is NUL-terminated and parent remains open for this call.
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a fresh descriptor owned by this File.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn stat_at(parent: RawFd, name: &CStr) -> io::Result<libc::stat> {
    let mut metadata = std::mem::MaybeUninit::uninit();
    // SAFETY: metadata is writable, name is NUL-terminated, and parent is open.
    if unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstatat initialized the complete stat structure.
    Ok(unsafe { metadata.assume_init() })
}

struct Directory(*mut libc::DIR);
impl Directory {
    fn new(fd: RawFd) -> io::Result<Self> {
        // SAFETY: fd is borrowed from an open File. dup creates an owned descriptor.
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: duplicate is a fresh descriptor owned by this value.
        let owned = unsafe { OwnedFd::from_raw_fd(duplicate) };
        let directory_fd = owned.into_raw_fd();
        // SAFETY: fdopendir takes ownership only on success.
        let directory = unsafe { libc::fdopendir(directory_fd) };
        if directory.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: fdopendir did not take ownership because it failed.
            unsafe { libc::close(directory_fd) };
            return Err(error);
        }
        Ok(Self(directory))
    }
    fn next(&self) -> io::Result<Option<CString>> {
        // SAFETY: errno is thread-local and readdir receives a live DIR owned by self.
        unsafe {
            #[cfg(target_os = "macos")]
            let errno = libc::__error();
            #[cfg(target_os = "linux")]
            let errno = libc::__errno_location();
            *errno = 0;
            let entry = libc::readdir(self.0);
            if entry.is_null() {
                return if *errno == 0 {
                    Ok(None)
                } else {
                    Err(io::Error::last_os_error())
                };
            }
            Ok(Some(CStr::from_ptr((*entry).d_name.as_ptr()).to_owned()))
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: this DIR is owned by self and closed exactly once.
        unsafe {
            libc::closedir(self.0);
        }
    }
}
