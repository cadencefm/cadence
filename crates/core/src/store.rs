//! Atomic file writes and bounded file reads.
//!
//! All project, preset and take persistence goes through one buffered atomic
//! write (size-checked before touching disk, temp file, fsync of file and
//! parent directory, rename) and one bounded reader.
use crate::material::{parse_document_json, write_document_json, Project};
use crate::mixer::LiveFlags;
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static ATOMIC_COUNTER: AtomicU64 = AtomicU64::new(0);
static TAKE_COUNTER: AtomicU64 = AtomicU64::new(0);
pub const MAX_RECOVERY_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_UNKEPT_TAKES: usize = 20;

/// Read a file up to `max_bytes`, failing before allocating unbounded memory.
pub fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    let meta_len = file
        .metadata()
        .with_context(|| format!("Reading metadata for {}", path.display()))?
        .len();
    ensure!(
        meta_len <= max_bytes,
        "{} exceeds {max_bytes} byte limit",
        path.display()
    );
    let mut bytes = Vec::with_capacity(meta_len.min(1024 * 1024) as usize);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("Reading {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= max_bytes,
        "{} exceeds {max_bytes} byte limit",
        path.display()
    );
    Ok(bytes)
}

/// Metadata for one directory entry returned by [`read_dir_entries`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub path: PathBuf,
    pub file_name: String,
    pub is_dir: bool,
    pub is_file: bool,
}

pub fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().context("Reading current working directory")
}

/// Resolve the user's home directory from `HOME` (or `USERPROFILE` on Windows).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Expand a leading `~` or `~/` using `HOME` (or `USERPROFILE`), leaving other paths unchanged.
pub fn expand_tilde(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        if let Some(dir) = home_dir() {
            return dir;
        }
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        if let Some(dir) = home_dir() {
            return dir.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub fn is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub fn is_file(path: &Path) -> bool {
    path.is_file()
}

pub fn file_len(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("Reading metadata for {}", path.display()))?
        .len())
}

pub fn canonicalize(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("Canonicalizing {}", path.display()))
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("Creating {}", path.display()))
}

pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("Removing {}", path.display())),
    }
}

pub fn read_dir_entries(path: &Path) -> Result<Vec<DirEntryInfo>> {
    let read =
        fs::read_dir(path).with_context(|| format!("Reading directory {}", path.display()))?;
    let mut entries = Vec::new();
    const MAX_DIR_SCAN_ENTRIES: usize = 4_096;
    for entry in read.take(MAX_DIR_SCAN_ENTRIES).flatten() {
        let p = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        entries.push(DirEntryInfo {
            is_dir: p.is_dir(),
            is_file: p.is_file(),
            path: p,
            file_name,
        });
    }
    Ok(entries)
}

/// Write raw `payload` bytes atomically through a sibling temp file, refusing
/// writes over `max_bytes` before touching disk.
pub fn write_atomic_bytes(path: &Path, max_bytes: u64, payload: &[u8]) -> Result<()> {
    ensure!(
        payload.len() as u64 <= max_bytes,
        "Refusing to write {} ({} bytes exceeds {max_bytes} byte limit)",
        path.display(),
        payload.len()
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        fs::create_dir_all(parent).with_context(|| format!("Creating {}", parent.display()))?;
    }
    let name = path
        .file_name()
        .context("Destination needs a filename")?
        .to_string_lossy();
    let temp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        ATOMIC_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .with_context(|| format!("Creating temporary file for {}", path.display()))?;
    let result = (|| -> Result<()> {
        {
            let mut buffered = BufWriter::new(&mut file);
            buffered.write_all(payload)?;
            buffered.flush()?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        sync_directory(parent);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.with_context(|| format!("Writing {}", path.display()))
}

/// Write `path` atomically through a sibling temp file, refusing writes over
/// `max_bytes` before touching disk. Syncs both the file and its parent directory.
pub fn write_atomic(
    path: &Path,
    max_bytes: u64,
    write_fn: impl FnOnce(&mut Vec<u8>) -> Result<()>,
) -> Result<()> {
    let mut payload = Vec::new();
    write_fn(&mut payload)?;
    write_atomic_bytes(path, max_bytes, &payload)
}

#[cfg(unix)]
fn sync_directory(dir: &Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) {}

/// Per-project session state directory holding an exclusive instance lock,
/// crash-recovery autosave (`recovery.json`), and auto-recorded session takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionDir {
    root: PathBuf,
}

/// Exclusive process lock guarding a [`SessionDir`]. Removed on drop.
#[derive(Debug)]
pub struct InstanceLock {
    path: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl SessionDir {
    /// Use an explicit session directory (primarily for tests and `--session-dir`).
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve the platform state directory for a project file path.
    pub fn for_project(project_path: &Path) -> Result<Self> {
        let canonical = project_path
            .canonicalize()
            .or_else(|_| {
                let abs = if project_path.is_absolute() {
                    project_path.to_path_buf()
                } else {
                    std::env::current_dir()?.join(project_path)
                };
                Ok::<PathBuf, std::io::Error>(abs)
            })
            .with_context(|| format!("Resolving session key for {}", project_path.display()))?;
        let stem: String = canonical
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .take(32)
            .collect();
        let hash = fnv1a_64(canonical.to_string_lossy().as_bytes());
        let base = platform_state_dir()?;
        Ok(Self {
            root: base.join("sessions").join(format!("{stem}-{hash:016x}")),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("lock")
    }

    pub fn recovery_path(&self) -> PathBuf {
        self.root.join("recovery.json")
    }

    pub fn takes_dir(&self) -> PathBuf {
        self.root.join("takes")
    }

    /// Safe maximum byte length for a Unix domain socket path (`sun_path` is 104 bytes on macOS).
    #[cfg(unix)]
    pub const MAX_UNIX_SOCKET_PATH_BYTES: usize = 96;

    pub fn ctl_socket_path(&self) -> PathBuf {
        let direct = self.root.join("ctl.sock");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if direct.as_os_str().as_bytes().len() > Self::MAX_UNIX_SOCKET_PATH_BYTES {
                let hash = fnv1a_64(self.root.as_os_str().as_bytes());
                let name = format!("cadence-ctl-{hash:016x}.sock");
                let candidate = std::env::temp_dir().join(&name);
                if candidate.as_os_str().as_bytes().len() <= Self::MAX_UNIX_SOCKET_PATH_BYTES {
                    return candidate;
                }
                return PathBuf::from("/tmp").join(name);
            }
        }
        direct
    }

    pub fn ear_wav_path(&self) -> PathBuf {
        self.root.join("ear.wav")
    }

    /// Root directory containing all per-project session directories on this host.
    pub fn sessions_dir() -> Result<PathBuf> {
        Ok(platform_state_dir()?.join("sessions"))
    }

    /// Acquire the exclusive session lock, clearing any stale lock left by a dead PID.
    pub fn acquire_lock(&self) -> Result<InstanceLock> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("Creating session directory {}", self.root.display()))?;
        let lock_path = self.lock_path();
        let pid = std::process::id();
        for attempt in 0..2 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    writeln!(file, "{pid}")?;
                    file.sync_all()?;
                    sync_directory(&self.root);
                    return Ok(InstanceLock { path: lock_path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = read_lock_pid(&lock_path);
                    if let Some(holder_pid) = holder {
                        if is_pid_alive(holder_pid) {
                            bail!(
                                "Another Cadence instance (pid {holder_pid}) holds the session lock at {}",
                                lock_path.display()
                            );
                        }
                    }
                    if attempt == 0 {
                        let _ = fs::remove_file(&lock_path);
                        continue;
                    }
                    bail!("Could not acquire session lock at {}", lock_path.display());
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("Creating session lock {}", lock_path.display()));
                }
            }
        }
        bail!("Could not acquire session lock at {}", lock_path.display())
    }

    /// Atomically write a crash-recovery snapshot without touching the user's saved project.
    pub fn write_recovery(&self, project: &Project, flags: &LiveFlags) -> Result<()> {
        project.validate()?;
        write_atomic(&self.recovery_path(), MAX_RECOVERY_BYTES, |buf| {
            write_document_json(&mut *buf, project, flags)
        })
    }

    pub fn has_recovery(&self) -> bool {
        self.recovery_path().is_file()
    }

    /// Load and validate a crash-recovery snapshot if one exists.
    pub fn load_recovery(&self) -> Result<Option<(Project, LiveFlags)>> {
        let path = self.recovery_path();
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = read_bounded(&path, MAX_RECOVERY_BYTES)?;
        let (project, flags) = parse_document_json(&bytes).context("Invalid recovery snapshot")?;
        Ok(Some((project, flags)))
    }

    pub fn clear_recovery(&self) {
        let _ = fs::remove_file(self.recovery_path());
    }

    /// Allocate a unique path for a new auto-recorded session take.
    pub fn next_take_path(&self) -> Result<PathBuf> {
        next_session_take_path(&self.takes_dir())
    }

    /// Retain the newest `max_keep` unkept take journals in `<session>/takes/`.
    pub fn prune_takes(&self, max_keep: usize) -> Result<usize> {
        prune_takes_dir(&self.takes_dir(), max_keep)
    }
}

/// Allocate a unique `.take.jsonl` path inside `takes_dir`, creating the directory if needed.
pub fn next_session_take_path(takes_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(takes_dir)
        .with_context(|| format!("Creating takes directory {}", takes_dir.display()))?;
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = TAKE_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(takes_dir.join(format!("take-{ms:016}-{pid}-{seq:04}.take.jsonl")))
}

/// Keep the `max_keep` newest `.take.jsonl` files in `takes_dir` and delete older ones.
pub fn prune_takes_dir(takes_dir: &Path, max_keep: usize) -> Result<usize> {
    const MAX_PRUNE_SCAN_ENTRIES: usize = 4_096;
    if !takes_dir.is_dir() {
        return Ok(0);
    }
    let mut files: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(takes_dir)
        .with_context(|| format!("Reading takes directory {}", takes_dir.display()))?
        .take(MAX_PRUNE_SCAN_ENTRIES)
    {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_file() && name.ends_with(".take.jsonl") {
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            files.push((modified, name, path));
        }
    }
    if files.len() <= max_keep {
        return Ok(0);
    }
    // Sort newest first (tie-breaking on lexicographical timestamp filename).
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let mut removed = 0;
    for (_, _, old) in files.into_iter().skip(max_keep) {
        if fs::remove_file(&old).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn platform_state_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.trim().is_empty() {
            return Ok(PathBuf::from(xdg).join("cadence"));
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return Ok(PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("cadence"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return Ok(PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("cadence"));
        }
    }
    Ok(std::env::temp_dir().join("cadence"))
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn read_lock_pid(path: &Path) -> Option<u32> {
    let bytes = read_bounded(path, 64).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    text.trim().parse::<u32>().ok()
}

#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if pid == std::process::id() {
        return true;
    }
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(unix))]
fn is_pid_alive(pid: u32) -> bool {
    pid == std::process::id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_enforces_size_limit_and_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.json");
        write_atomic(&path, 64, |buf| {
            buf.extend_from_slice(b"{\"ok\":true}\n");
            Ok(())
        })
        .unwrap();
        assert_eq!(read_bounded(&path, 64).unwrap(), b"{\"ok\":true}\n");
        assert!(read_bounded(&path, 4).is_err());

        // Oversized write fails before modifying the existing file.
        let err = write_atomic(&path, 8, |buf| {
            buf.extend_from_slice(b"0123456789abcdef");
            Ok(())
        });
        assert!(err.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{\"ok\":true}\n");
    }

    #[test]
    fn instance_lock_blocks_live_holder_and_clears_stale_pid() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionDir::at(dir.path().join("session"));
        let lock = session.acquire_lock().unwrap();
        assert!(session.lock_path().is_file());
        // Second acquisition while live process holds it must fail.
        assert!(session.acquire_lock().is_err());
        drop(lock);
        assert!(!session.lock_path().exists());

        // Simulate a crash leaving a stale dead PID in the lock file.
        fs::write(session.lock_path(), "99999999\n").unwrap();
        let lock2 = session.acquire_lock().unwrap();
        assert_eq!(
            read_lock_pid(&session.lock_path()),
            Some(std::process::id())
        );
        drop(lock2);
    }

    #[test]
    fn recovery_snapshot_roundtrips_and_prune_keeps_newest_takes() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionDir::at(dir.path().join("session"));
        let mut project = Project::demo();
        project.bpm = 142.0;
        let flags = LiveFlags::default();

        assert!(!session.has_recovery());
        session.write_recovery(&project, &flags).unwrap();
        assert!(session.has_recovery());
        let raw = fs::read(session.recovery_path()).unwrap();
        assert!(
            raw.ends_with(b"\n") && !raw.ends_with(b"\n\n"),
            "recovery.json should end with exactly one trailing newline"
        );
        let (loaded, loaded_flags) = session.load_recovery().unwrap().unwrap();
        assert_eq!(loaded.bpm, 142.0);
        assert_eq!(loaded_flags, flags);
        session.clear_recovery();
        assert!(!session.has_recovery());

        // Create 25 takes and prune down to 20.
        let mut paths = Vec::new();
        for _ in 0..25 {
            let p = session.next_take_path().unwrap();
            fs::write(&p, b"{}\n").unwrap();
            paths.push(p);
        }
        let removed = session.prune_takes(20).unwrap();
        assert_eq!(removed, 5);
        let remaining = fs::read_dir(session.takes_dir()).unwrap().count();
        assert_eq!(remaining, 20);
        // The newest path must still exist.
        assert!(paths.last().unwrap().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn ctl_socket_path_falls_back_under_sun_len_deterministically() {
        use std::os::unix::ffi::OsStrExt;
        let short = SessionDir::at(PathBuf::from("/tmp/cadence-short"));
        assert_eq!(
            short.ctl_socket_path(),
            PathBuf::from("/tmp/cadence-short/ctl.sock")
        );

        let long_root = PathBuf::from(format!("/tmp/{}/session", "very-long-segment-".repeat(8)));
        let long = SessionDir::at(long_root.clone());
        let sock = long.ctl_socket_path();
        assert!(
            sock.as_os_str().as_bytes().len() <= SessionDir::MAX_UNIX_SOCKET_PATH_BYTES,
            "fallback socket path must stay within Unix SUN_LEN: {}",
            sock.display()
        );
        assert_eq!(
            sock,
            SessionDir::at(long_root).ctl_socket_path(),
            "fallback socket path must be deterministic across server and client"
        );
    }

    #[test]
    fn expand_tilde_resolves_home_prefix_and_preserves_other_paths() {
        assert_eq!(
            expand_tilde("relative/samples"),
            PathBuf::from("relative/samples")
        );
        assert_eq!(
            expand_tilde("  /abs/presets  "),
            PathBuf::from("/abs/presets")
        );
        if let Some(home) = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
        {
            assert_eq!(expand_tilde("~"), home);
            assert_eq!(expand_tilde("~/Samples/Drums"), home.join("Samples/Drums"));
        }
    }
}
