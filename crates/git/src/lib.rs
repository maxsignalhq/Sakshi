//! What did a session change? Answered by running the local `git` binary.
//!
//! A [`Baseline`] snapshots the working tree when a session starts, so a later
//! [`Baseline::diff`] shows only what happened since, not the uncommitted work
//! that was already there. Everything is local: `git` is run read-only, with
//! optional locks off so it never gets in the agent's way. The one thing that
//! writes is [`add_worktree`], and only when the user asks for it.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sakshi_core::{parse_unified_diff, ChangeKind, Diff, FileChange};

/// New files larger than this are listed but not read.
const MAX_NEW_FILE_BYTES: u64 = 256 * 1024;

/// At most this many new files are read per diff.
const MAX_NEW_FILES: usize = 200;

/// The state of a repository when a session started.
#[derive(Debug, Clone)]
pub struct Baseline {
    root: PathBuf,
    /// Commit to diff against: a snapshot of the working tree, or `HEAD`.
    /// `None` in a repository without commits.
    rev: Option<String>,
    /// Untracked files that already existed.
    untracked: HashSet<String>,
}

fn git(root: &Path, args: &[&str]) -> io::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

fn untracked_files(root: &Path) -> io::Result<HashSet<String>> {
    let out = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

impl Baseline {
    /// Snapshot the repository containing `dir`, or `None` if `dir` is not in
    /// a git repository (or git is not installed).
    pub fn capture(dir: &Path) -> Option<Baseline> {
        let root = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"]).ok()?.trim());
        let head = git(&root, &["rev-parse", "--verify", "-q", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // `stash create` records the working tree as a commit without touching
        // it. It prints nothing when the tree is clean. The identity is passed
        // explicitly because the commit needs one and the user may have none.
        let snapshot = git(
            &root,
            &[
                "-c",
                "user.name=sakshi",
                "-c",
                "user.email=sakshi@localhost",
                "stash",
                "create",
            ],
        )
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
        Some(Baseline {
            untracked: untracked_files(&root).unwrap_or_default(),
            rev: snapshot.or(head),
            root,
        })
    }

    /// Everything that changed since the baseline was captured.
    pub fn diff(&self) -> io::Result<Diff> {
        let mut diff = match &self.rev {
            Some(rev) => parse_unified_diff(&git(
                &self.root,
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-U0",
                    rev,
                ],
            )?),
            None => Diff::default(),
        };
        let mut created: Vec<String> = untracked_files(&self.root)?
            .difference(&self.untracked)
            .cloned()
            .collect();
        created.sort();
        for path in created.into_iter().take(MAX_NEW_FILES) {
            diff.files.push(self.read_new_file(path));
        }
        Ok(diff)
    }

    fn read_new_file(&self, path: String) -> FileChange {
        let mut change = FileChange::new(path.clone(), ChangeKind::Added);
        let full = self.root.join(&path);
        let small = std::fs::metadata(&full).is_ok_and(|m| m.len() <= MAX_NEW_FILE_BYTES);
        if small {
            if let Ok(bytes) = std::fs::read(&full) {
                // Binary files are listed without lines.
                if !bytes.contains(&0) {
                    change.added = String::from_utf8_lossy(&bytes)
                        .lines()
                        .map(str::to_string)
                        .collect();
                }
            }
        }
        change
    }
}

/// A separate checkout created for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// The original repository's root.
    pub repo: PathBuf,
    /// The new checkout's root.
    pub path: PathBuf,
    /// The directory to start in: `path`, plus the sub-folder `dir` was in.
    pub cwd: PathBuf,
    /// The new branch checked out there.
    pub branch: String,
}

/// Check out a new branch `sakshi/<tag>` from `HEAD` into a sibling folder of
/// the repository containing `dir`, so a session can work without touching the
/// main checkout. Uncommitted changes are not carried over. Nothing is merged
/// or removed afterwards: that stays with the user.
pub fn add_worktree(dir: &Path, tag: &str) -> io::Result<Worktree> {
    let repo = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
    let name = repo
        .file_name()
        .map_or_else(|| "repo".to_string(), |n| n.to_string_lossy().into_owned());
    let path = repo.with_file_name(format!("{name}-sakshi-{tag}"));
    let branch = format!("sakshi/{tag}");
    let path_arg = path.to_string_lossy().into_owned();
    git(&repo, &["worktree", "add", "-q", "-b", &branch, &path_arg])?;
    // Ask git for the sub-folder instead of comparing paths, which differ in
    // spelling between symlinks (`/var` on macOS) and Windows short names.
    let prefix = git(dir, &["rev-parse", "--show-prefix"])?;
    let cwd = path.join(prefix.trim());
    Ok(Worktree {
        repo,
        path,
        cwd,
        branch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sakshi-git-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }

        fn git(&self, args: &[&str]) {
            let status = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        }

        /// A repository with one committed file, `a.txt`, holding two lines.
        fn repo(tag: &str) -> Self {
            let s = Self::new(tag);
            s.git(&["init", "-q"]);
            s.write("a.txt", "one\ntwo\n");
            s.git(&["add", "."]);
            s.git(&["commit", "-q", "-m", "init"]);
            s
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn paths(diff: &Diff) -> Vec<&str> {
        diff.files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn a_worktree_is_a_separate_checkout_on_a_new_branch() {
        let repo = Scratch::repo("wt");
        repo.write("sub/b.txt", "b\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", "sub"]);
        repo.write("a.txt", "uncommitted\n");

        let sub = repo.path().join("sub");
        let wt = add_worktree(&sub, "t1").unwrap();
        let _cleanup = Scratch(wt.path.clone());

        assert_eq!(wt.branch, "sakshi/t1");
        assert!(wt.path.join("a.txt").exists());
        assert_eq!(wt.cwd, wt.path.join("sub"));
        // Starts from HEAD: the uncommitted edit is not carried over.
        assert_eq!(
            std::fs::read_to_string(wt.path.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        // The original is untouched, and the new branch exists.
        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "uncommitted\n"
        );
        let branches = git(repo.path(), &["branch", "--list", "sakshi/t1"]).unwrap();
        assert!(branches.contains("sakshi/t1"));
        // Edits there are seen as changes by a baseline of the worktree only.
        let base = Baseline::capture(&wt.cwd).unwrap();
        std::fs::write(wt.path.join("new.txt"), "x\n").unwrap();
        assert_eq!(paths(&base.diff().unwrap()), ["new.txt"]);
    }

    #[test]
    fn a_worktree_needs_a_repository_and_a_free_branch_name() {
        let plain = Scratch::new("wt-norepo");
        assert!(add_worktree(plain.path(), "x").is_err());

        let repo = Scratch::repo("wt-dup");
        let first = add_worktree(repo.path(), "dup").unwrap();
        let _cleanup = Scratch(first.path.clone());
        assert!(add_worktree(repo.path(), "dup").is_err());
    }

    #[test]
    fn outside_a_repository_there_is_no_baseline() {
        let dir = Scratch::new("norepo");
        assert!(Baseline::capture(dir.path()).is_none());
    }

    #[test]
    fn a_clean_repository_has_an_empty_diff() {
        let repo = Scratch::repo("clean");
        let baseline = Baseline::capture(repo.path()).unwrap();
        assert!(baseline.diff().unwrap().files.is_empty());
    }

    #[test]
    fn edits_to_tracked_files_show_their_lines() {
        let repo = Scratch::repo("edit");
        let baseline = Baseline::capture(repo.path()).unwrap();
        repo.write("a.txt", "one\nTWO\nthree\n");
        let diff = baseline.diff().unwrap();
        assert_eq!(paths(&diff), ["a.txt"]);
        assert_eq!(diff.files[0].kind, ChangeKind::Modified);
        assert_eq!(diff.files[0].removed, ["two"]);
        assert_eq!(diff.files[0].added, ["TWO", "three"]);
    }

    #[test]
    fn work_that_predates_the_session_is_not_attributed_to_it() {
        let repo = Scratch::repo("predate");
        repo.write("a.txt", "one\ntwo\nbefore-session\n");
        repo.write("early.txt", "was already here\n");
        let baseline = Baseline::capture(repo.path()).unwrap();
        assert!(baseline.diff().unwrap().files.is_empty());

        repo.write("a.txt", "one\ntwo\nbefore-session\nduring-session\n");
        let diff = baseline.diff().unwrap();
        assert_eq!(paths(&diff), ["a.txt"]);
        assert_eq!(diff.files[0].added, ["during-session"]);
    }

    #[test]
    fn new_files_appear_as_added_with_their_lines() {
        let repo = Scratch::repo("newfile");
        let baseline = Baseline::capture(repo.path()).unwrap();
        repo.write("src/new.rs", "fn main() {}\n// hi\n");
        let diff = baseline.diff().unwrap();
        assert_eq!(paths(&diff), ["src/new.rs"]);
        assert_eq!(diff.files[0].kind, ChangeKind::Added);
        assert_eq!(diff.files[0].added, ["fn main() {}", "// hi"]);
    }

    #[test]
    fn binary_new_files_are_listed_without_lines() {
        let repo = Scratch::repo("binary");
        let baseline = Baseline::capture(repo.path()).unwrap();
        std::fs::write(repo.path().join("blob.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        let diff = baseline.diff().unwrap();
        assert_eq!(paths(&diff), ["blob.bin"]);
        assert!(diff.files[0].added.is_empty());
    }

    #[test]
    fn gitignored_files_are_not_listed() {
        let repo = Scratch::repo("ignored");
        repo.write(".gitignore", "secret.txt\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", "ignore"]);
        let baseline = Baseline::capture(repo.path()).unwrap();
        repo.write("secret.txt", "x\n");
        assert!(baseline.diff().unwrap().files.is_empty());
    }

    #[test]
    fn deleted_files_are_reported_as_deleted() {
        let repo = Scratch::repo("delete");
        let baseline = Baseline::capture(repo.path()).unwrap();
        std::fs::remove_file(repo.path().join("a.txt")).unwrap();
        let diff = baseline.diff().unwrap();
        assert_eq!(paths(&diff), ["a.txt"]);
        assert_eq!(diff.files[0].kind, ChangeKind::Deleted);
        assert_eq!(diff.files[0].removed, ["one", "two"]);
    }

    #[test]
    fn a_repository_without_commits_still_reports_new_files() {
        let repo = Scratch::new("nocommits");
        repo.git(&["init", "-q"]);
        let baseline = Baseline::capture(repo.path()).unwrap();
        repo.write("first.txt", "hello\n");
        let diff = baseline.diff().unwrap();
        assert_eq!(paths(&diff), ["first.txt"]);
    }

    #[test]
    fn capturing_from_a_subdirectory_uses_the_repository_root() {
        let repo = Scratch::repo("subdir");
        repo.write("deep/dir/x.txt", "x\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", "deep"]);
        let baseline = Baseline::capture(&repo.path().join("deep/dir")).unwrap();
        repo.write("a.txt", "changed\n");
        let diff = baseline.diff().unwrap();
        assert_eq!(paths(&diff), ["a.txt"]);
    }
}
