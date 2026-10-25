//! Where a Sakshi process listens: a socket file on Unix, a named pipe on Windows.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use sakshi_core::SessionId;

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
/// crashed one leaves nothing that blocks the next. The directory is created
/// atomically under a name nobody can predict, and fails rather than adopt
/// anything that already exists: in a shared temp directory another user must
/// not be able to plant the directory first and then swap the files in it.
/// Removed on drop.
#[derive(Debug)]
pub struct RuntimeDir {
    path: PathBuf,
}

impl RuntimeDir {
    /// Create a fresh `sakshi-<pid>-<random>` directory under
    /// `$XDG_RUNTIME_DIR` or the system temp dir, readable by the current user
    /// only on Unix.
    pub fn create() -> io::Result<Self> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        Self::create_unique(&base, "sakshi")
    }

    /// Create a new directory under `base` named `<prefix>-<pid>-<random>`.
    /// Never reuses or replaces an existing path.
    pub fn create_unique(base: &Path, prefix: &str) -> io::Result<Self> {
        for _ in 0..16 {
            let random = SessionId::generate().0 as u32;
            let name = format!("{prefix}-{}-{random:08x}", std::process::id());
            let path = base.join(name);
            match create_private_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not find an unused runtime directory name",
        ))
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
    // Not recursive: creating an existing path must fail, never succeed.
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_dir_is_created_and_removed() {
        let path;
        {
            let dir = RuntimeDir::create_unique(&std::env::temp_dir(), "sakshi-test-dir").unwrap();
            path = dir.path().to_path_buf();
            assert!(path.is_dir());
        }
        assert!(!path.exists());
    }

    #[test]
    fn each_directory_gets_its_own_name_and_endpoint() {
        let base = std::env::temp_dir();
        let a = RuntimeDir::create_unique(&base, "sakshi-test-uniq").unwrap();
        let b = RuntimeDir::create_unique(&base, "sakshi-test-uniq").unwrap();
        assert_ne!(a.path(), b.path());
        assert_ne!(a.endpoint(), b.endpoint());
    }

    #[test]
    fn an_existing_path_is_never_adopted() {
        // The creation primitive itself must refuse a path that exists, which
        // is what stops a planted directory from being reused.
        let base = std::env::temp_dir();
        let planted = base.join(format!("sakshi-test-planted-{}", std::process::id()));
        std::fs::create_dir_all(&planted).unwrap();
        let err = create_private_dir(&planted).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        std::fs::remove_dir_all(&planted).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = RuntimeDir::create_unique(&std::env::temp_dir(), "sakshi-test-mode").unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn endpoint_round_trips_through_raw() {
        let e = Endpoint::from_raw("abc");
        assert_eq!(e.as_str(), "abc");
        assert_eq!(e.to_string(), "abc");
    }
}
