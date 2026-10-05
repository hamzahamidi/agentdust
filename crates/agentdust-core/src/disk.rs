use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
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
                        category,
                        0,
                        classify,
                        false,
                        false,
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

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        file: &File,
        device: u64,
        category: Category,
        depth: usize,
        classify: bool,
        projects: bool,
        cache: bool,
        result: &mut RootReport,
    ) {
        if depth >= MAX_DEPTH {
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
            self.entries += 1;
            let child_category = if classify && depth == 0 {
                classify_name(name.as_bytes())
            } else if classify && depth == 1 && cache && name.as_bytes() == b"changelog.md" {
                Category::Rebuildable
            } else if projects && depth == 2 && name.as_bytes() == b"memory" {
                Category::ApplicationState
            } else {
                category
            };
            let metadata = match stat_at(file.as_raw_fd(), &name) {
                Ok(metadata) => metadata,
                Err(_) => {
                    result.skipped.errors += 1;
                    continue;
                }
            };
            let kind = metadata.st_mode & libc::S_IFMT;
            if kind == libc::S_IFLNK {
                result.skipped.symlinks += 1;
                continue;
            }
            if kind != libc::S_IFREG && kind != libc::S_IFDIR {
                result.skipped.special_files += 1;
                continue;
            }
            if metadata.st_dev as u64 != device {
                result.skipped.other_filesystems += 1;
                continue;
            }
            if self.seen.contains(&(metadata.st_dev as u64, metadata.st_ino)) {
                result.skipped.duplicate_inodes += 1;
                continue;
            }
            if kind == libc::S_IFDIR {
                match open_at(file.as_raw_fd(), &name).and_then(|child| {
                    let actual = child.metadata()?;
                    if actual.dev() != metadata.st_dev as u64 || actual.ino() != metadata.st_ino {
                        return Err(io::Error::other("directory changed"));
                    }
                    Ok((child, actual))
                }) {
                    Ok((child, actual)) => {
                        self.seen.insert((actual.dev(), actual.ino()));
                        add(result, child_category, &actual);
                        self.walk(
                            &child,
                            device,
                            child_category,
                            depth + 1,
                            classify,
                            projects || (classify && depth == 0 && name.as_bytes() == b"projects"),
                            classify && depth == 0 && name.as_bytes() == b"cache",
                            result,
                        );
                    }
                    Err(_) => result.skipped.errors += 1,
                }
            } else {
                self.seen.insert((metadata.st_dev as u64, metadata.st_ino));
                let usage = &mut result.usage[child_category as usize];
                usage.files += 1;
                usage.logical_bytes = usage.logical_bytes.saturating_add(metadata.st_size.max(0) as u64);
                usage.allocated_bytes = usage
                    .allocated_bytes
                    .saturating_add((metadata.st_blocks.max(0) as u64).saturating_mul(512));
            }
        }
    }
}

fn classify_name(name: &[u8]) -> Category {
    match name {
        b"remote-settings.json" | b"policy-limits.json" | b"policy-limits.json.stamp.json" => {
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
        // SAFETY: duplicate is a fresh descriptor, released on failure or transferred to fdopendir.
        let owned = unsafe { OwnedFd::from_raw_fd(duplicate) };
        // SAFETY: fdopendir takes ownership only on success.
        let directory = unsafe { libc::fdopendir(owned.as_raw_fd()) };
        if directory.is_null() {
            return Err(io::Error::last_os_error());
        }
        std::mem::forget(owned);
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
