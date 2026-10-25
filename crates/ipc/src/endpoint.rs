//! Where a Sakshi process listens: a socket file on Unix, a named pipe on Windows.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Environment variable carrying the endpoint to child processes.
pub const ENDPOINT_ENV: &str = "SAKSHI_ENDPOINT";

/// Environment variable carrying the session id to child processes.
pub const SESSION_ENV: &str = "SAKSHI_SESSION_ID";

/// Address of an IPC listener, in the platform's native spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint(String);

impl Endpoint {
    /// Wrap a raw endpoint string, e.g. one read from [`ENDPOINT_ENV`].
    pub fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The raw endpoint string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A private per-process directory for the socket and generated config files.
///
/// Every Sakshi process gets its own, so several can run side by side and a
/// crashed one leaves nothing that blocks the next. Removed on drop.
#[derive(Debug)]
pub struct RuntimeDir {
    path: PathBuf,
}

impl RuntimeDir {
    /// Create `sakshi-<pid>` under `$XDG_RUNTIME_DIR` or the system temp dir,
    /// readable by the current user only on Unix.
    pub fn create() -> io::Result<Self> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        Self::create_in(&base, &format!("sakshi-{}", std::process::id()))
    }

    /// Create the directory `name` under `base`, replacing a stale one.
    pub fn create_in(base: &Path, name: &str) -> io::Result<Self> {
        let path = base.join(name);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        create_private_dir(&path)?;
        Ok(Self { path })
    }

    /// The directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The IPC endpoint belonging to this directory.
    pub fn endpoint(&self) -> Endpoint {
        #[cfg(windows)]
        {
            let name = self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "sakshi".to_string());
            Endpoint(format!(r"\\.\pipe\{name}"))
        }
        #[cfg(not(windows))]
        {
            Endpoint(self.path.join("ipc.sock").to_string_lossy().into_owned())
        }
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        // Best effort: nothing useful to do if removal fails during shutdown.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_dir_is_created_and_removed() {
        let base = std::env::temp_dir();
        let name = format!("sakshi-test-{}-dir", std::process::id());
        let path;
        {
            let dir = RuntimeDir::create_in(&base, &name).unwrap();
            path = dir.path().to_path_buf();
            assert!(path.is_dir());
            assert!(dir.endpoint().as_str().contains(&name) || cfg!(unix));
        }
        assert!(!path.exists());
    }

    #[test]
    fn create_replaces_a_stale_directory() {
        let base = std::env::temp_dir();
        let name = format!("sakshi-test-{}-stale", std::process::id());
        std::fs::create_dir_all(base.join(&name)).unwrap();
        std::fs::write(base.join(&name).join("old"), b"x").unwrap();
        let dir = RuntimeDir::create_in(&base, &name).unwrap();
        assert!(!dir.path().join("old").exists());
    }

    #[test]
    fn endpoint_round_trips_through_raw() {
        let e = Endpoint::from_raw("abc");
        assert_eq!(e.as_str(), "abc");
        assert_eq!(e.to_string(), "abc");
    }
}
