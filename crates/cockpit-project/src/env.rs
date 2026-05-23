//! Injectable environment seams — filesystem, process spawning, clock.
//!
//! Plan §1.7 names this as a load-bearing architectural decision: all
//! non-determinism should be injectable via traits, with production callers
//! passing real implementations and tests passing fakes. This module is the
//! single place those traits live; `cockpit-testkit` re-exports the
//! [`fake`] submodule so test helpers and view-model tests share one set of
//! in-memory primitives.
//!
//! Why here and not in `cockpit-testkit`? The traits are production types
//! — the std-backed impls run in shipping code. `cockpit-testkit` only owns
//! the test-side conveniences (fixtures, fakes), so it depends on this
//! module rather than the other way around.
//!
//! The migration is incremental: every public function that currently uses
//! `std::fs` or `std::process::Command` is paired with a `_with` variant
//! that takes the trait objects. The unadorned function is a thin wrapper
//! that hands in the std-backed defaults so existing call sites keep
//! compiling unchanged (M4.10).

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Minimal filesystem facade — only the operations cockpit-project, the
/// project cache, and the format-on-save hook actually need. Keep this
/// narrow on purpose: a smaller surface is easier to fake than a full
/// `std::fs` mirror, and it stops creeping side-effects through the seam.
pub trait FileSystem: Send + Sync {
    /// Read a file's contents as UTF-8.
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    /// Atomically replace a file with `contents`.
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
    /// Create `path` and every missing parent directory.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// True when `path` exists and refers to a regular file.
    fn is_file(&self, path: &Path) -> bool;
    /// True when `path` exists and refers to a directory.
    fn is_dir(&self, path: &Path) -> bool;
}

/// Captured output of a spawned process — the bits cockpit actually looks
/// at. Matches `std::process::Output` shape without the underlying child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// True when the child exited with status `0`.
    pub success: bool,
    /// Raw stdout bytes.
    pub stdout: Vec<u8>,
    /// Raw stderr bytes.
    pub stderr: Vec<u8>,
}

impl ProcessOutput {
    /// Lossy UTF-8 view of `stdout` — convenient for status-line snippets.
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Lossy UTF-8 view of `stderr`.
    pub fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Spec for one process spawn: program, args, optional working directory.
/// Plain data so callers can build it once and reuse it (the format-on-save
/// retry loop, the mise-availability probe).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    /// Program name (resolved against `$PATH`).
    pub program: OsString,
    /// Arguments passed to the program, in order.
    pub args: Vec<OsString>,
    /// Working directory; `None` means inherit the parent's.
    pub current_dir: Option<PathBuf>,
}

impl ProcessSpec {
    /// New spec with no args and no working directory.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
        }
    }

    /// Append one argument, returning `self` for chaining.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments at once.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the working directory.
    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }
}

/// Process-spawning facade. The two methods cover everything cockpit needs
/// today: synchronous "run a thing and tell me how it exited" probes (mise
/// availability, git status) and "run a thing and give me its output"
/// captures (format-on-save).
pub trait ProcessRunner: Send + Sync {
    /// Spawn `spec`, wait for it to exit, and capture its output.
    fn run(&self, spec: &ProcessSpec) -> io::Result<ProcessOutput>;
}

/// Monotonic + wall-clock seam. Most cockpit code only needs `now()` for
/// debouncing; the wall-clock version exists for "last modified" displays
/// that may land later.
pub trait Clock: Send + Sync {
    /// A monotonic point used for measuring durations.
    fn now(&self) -> Instant;
    /// Wall-clock time — only call when you actually need calendar time.
    fn system_now(&self) -> SystemTime {
        SystemTime::now()
    }
}

// -- std-backed production impls -----------------------------------------

/// `FileSystem` backed by `std::fs`. The default production impl.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdFileSystem;

impl FileSystem for StdFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}

/// `ProcessRunner` backed by `std::process::Command`. The default
/// production impl.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdProcessRunner;

impl ProcessRunner for StdProcessRunner {
    fn run(&self, spec: &ProcessSpec) -> io::Result<ProcessOutput> {
        let mut command = std::process::Command::new(&spec.program);
        command.args(spec.args.iter().map(OsString::as_os_str));
        if let Some(dir) = spec.current_dir.as_deref() {
            command.current_dir(dir);
        }
        let output = command.output()?;
        Ok(ProcessOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// `Clock` backed by `std::time`. The default production impl.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdClock;

impl Clock for StdClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn system_now(&self) -> SystemTime {
        SystemTime::now()
    }
}

// -- fake impls for headless tests ---------------------------------------

/// In-memory `FileSystem` fake — path strings keyed in a hashmap. Designed
/// for hermetic unit tests in core crates (AGENTS §7) and the test cases
/// in [`crate::tests`] that exercise the format flow without spawning a
/// real process.
#[derive(Debug, Default)]
pub struct FakeFileSystem {
    inner: Mutex<FakeFsInner>,
}

#[derive(Debug, Default)]
struct FakeFsInner {
    files: std::collections::BTreeMap<PathBuf, Vec<u8>>,
    dirs: std::collections::BTreeSet<PathBuf>,
}

impl FakeFileSystem {
    /// New empty filesystem.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a file with the given UTF-8 contents.
    pub fn insert_file(&self, path: impl Into<PathBuf>, contents: &str) {
        let path = path.into();
        let mut inner = self.inner.lock().expect("FakeFileSystem poisoned");
        if let Some(parent) = path.parent() {
            inner.dirs.insert(parent.to_path_buf());
        }
        inner.files.insert(path, contents.as_bytes().to_vec());
    }

    /// Record `path` as an existing directory.
    pub fn insert_dir(&self, path: impl Into<PathBuf>) {
        let mut inner = self.inner.lock().expect("FakeFileSystem poisoned");
        inner.dirs.insert(path.into());
    }

    /// True when `path` was previously inserted as a file. Test helper.
    pub fn contains_file(&self, path: impl AsRef<Path>) -> bool {
        let inner = self.inner.lock().expect("FakeFileSystem poisoned");
        inner.files.contains_key(path.as_ref())
    }

    /// Snapshot the current contents of `path` (None when absent).
    pub fn snapshot(&self, path: impl AsRef<Path>) -> Option<String> {
        let inner = self.inner.lock().expect("FakeFileSystem poisoned");
        inner
            .files
            .get(path.as_ref())
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }
}

impl FileSystem for FakeFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let inner = self.inner.lock().expect("FakeFileSystem poisoned");
        match inner.files.get(path) {
            Some(bytes) => Ok(String::from_utf8_lossy(bytes).into_owned()),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("file not in fake fs: {}", path.display()),
            )),
        }
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut inner = self.inner.lock().expect("FakeFileSystem poisoned");
        if let Some(parent) = path.parent() {
            inner.dirs.insert(parent.to_path_buf());
        }
        inner.files.insert(path.to_path_buf(), contents.to_vec());
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        let mut inner = self.inner.lock().expect("FakeFileSystem poisoned");
        let mut acc = PathBuf::new();
        for component in path.components() {
            acc.push(component);
            inner.dirs.insert(acc.clone());
        }
        Ok(())
    }

    fn is_file(&self, path: &Path) -> bool {
        let inner = self.inner.lock().expect("FakeFileSystem poisoned");
        inner.files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        let inner = self.inner.lock().expect("FakeFileSystem poisoned");
        inner.dirs.contains(path)
    }
}

/// Scripted `ProcessRunner` — returns canned [`ProcessOutput`] per
/// `(program, first-arg)` lookup. Anything unscripted fails with
/// `ErrorKind::NotFound`, so tests have to opt into every spawn they expect.
#[derive(Debug, Default)]
pub struct FakeProcessRunner {
    inner: Mutex<FakeProcessInner>,
}

#[derive(Debug, Default)]
struct FakeProcessInner {
    /// Keyed by `(program, args)`; the args slice can be any prefix length
    /// so tests can match on `("mise", ["--version"])` etc.
    responses: Vec<(OsString, Vec<OsString>, ProcessOutput)>,
    /// Successful spawns recorded for assertions.
    log: Vec<ProcessSpec>,
}

impl FakeProcessRunner {
    /// New empty runner. Calls fail until `expect_*` is called.
    pub fn new() -> Self {
        Self::default()
    }

    /// Script a response for any spawn whose program is `program` and
    /// whose initial arguments match `args` (exact match, not prefix).
    pub fn expect<P, I, S>(&self, program: P, args: I, output: ProcessOutput)
    where
        P: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut inner = self.inner.lock().expect("FakeProcessRunner poisoned");
        inner.responses.push((
            program.as_ref().to_os_string(),
            args.into_iter()
                .map(|s| s.as_ref().to_os_string())
                .collect(),
            output,
        ));
    }

    /// Snapshot of every spawn the runner has accepted, in call order.
    pub fn spawns(&self) -> Vec<ProcessSpec> {
        let inner = self.inner.lock().expect("FakeProcessRunner poisoned");
        inner.log.clone()
    }
}

impl ProcessRunner for FakeProcessRunner {
    fn run(&self, spec: &ProcessSpec) -> io::Result<ProcessOutput> {
        let mut inner = self.inner.lock().expect("FakeProcessRunner poisoned");
        let matched = inner.responses.iter().position(|(program, args, _)| {
            program == &spec.program && args.as_slice() == spec.args.as_slice()
        });
        let output = match matched {
            Some(idx) => inner.responses.remove(idx).2,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "unscripted spawn: {} {}",
                        spec.program.to_string_lossy(),
                        spec.args
                            .iter()
                            .map(|a| a.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                ));
            }
        };
        inner.log.push(spec.clone());
        Ok(output)
    }
}

/// A controllable [`Clock`]. `advance` is the only mutator: tests step it
/// forward explicitly, mirroring `tokio::time::pause()` ergonomics.
#[derive(Debug)]
pub struct FakeClock {
    base: Instant,
    elapsed: Mutex<Duration>,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeClock {
    /// New clock pinned at `Instant::now()`.
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            elapsed: Mutex::new(Duration::ZERO),
        }
    }

    /// Push the virtual time forward by `dt`.
    pub fn advance(&self, dt: Duration) {
        let mut elapsed = self.elapsed.lock().expect("FakeClock poisoned");
        *elapsed += dt;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        let elapsed = *self.elapsed.lock().expect("FakeClock poisoned");
        self.base + elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_fs_round_trips_writes() {
        let fs = FakeFileSystem::new();
        let path = PathBuf::from("/tmp/x.toml");
        fs.write(&path, b"hello").unwrap();
        assert_eq!(fs.read_to_string(&path).unwrap(), "hello");
        assert!(fs.is_file(&path));
        assert!(!fs.is_dir(&path));
    }

    #[test]
    fn fake_fs_reports_missing_files() {
        let fs = FakeFileSystem::new();
        let err = fs.read_to_string(Path::new("/missing")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn fake_fs_create_dir_all_records_every_parent() {
        let fs = FakeFileSystem::new();
        fs.create_dir_all(Path::new("/a/b/c")).unwrap();
        assert!(fs.is_dir(Path::new("/a")));
        assert!(fs.is_dir(Path::new("/a/b")));
        assert!(fs.is_dir(Path::new("/a/b/c")));
    }

    #[test]
    fn fake_process_runner_serves_scripted_responses_once() {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "mise",
            ["--version"],
            ProcessOutput {
                success: true,
                stdout: b"mise 2026.0.0\n".to_vec(),
                stderr: Vec::new(),
            },
        );
        let spec = ProcessSpec::new("mise").arg("--version");
        let out = runner.run(&spec).unwrap();
        assert!(out.success);
        assert!(out.stdout_string().starts_with("mise "));

        // A second call without re-scripting must fail rather than reuse.
        let err = runner.run(&spec).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn fake_process_runner_logs_calls_in_order() {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "mise",
            ["run", "fmt"],
            ProcessOutput {
                success: true,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        );
        let spec = ProcessSpec::new("mise")
            .arg("run")
            .arg("fmt")
            .current_dir("/proj");
        runner.run(&spec).unwrap();
        let log = runner.spawns();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0], spec);
    }

    #[test]
    fn fake_clock_advances_monotonically() {
        let clock = FakeClock::new();
        let t0 = clock.now();
        clock.advance(Duration::from_millis(50));
        let t1 = clock.now();
        clock.advance(Duration::from_millis(50));
        let t2 = clock.now();
        assert!(t1 > t0);
        assert!(t2 > t1);
        assert_eq!(t2.duration_since(t0), Duration::from_millis(100));
    }
}
