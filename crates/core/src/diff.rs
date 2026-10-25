//! A small model of a code diff, and a parser for `git diff -U0` output.

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The file is new.
    Added,
    /// The file's contents changed.
    Modified,
    /// The file was removed.
    Deleted,
    /// The file moved, possibly with edits.
    Renamed,
}

/// The changes to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the repository root, using `/` separators.
    pub path: String,
    /// What happened to the file.
    pub kind: ChangeKind,
    /// Lines added, without the leading `+`.
    pub added: Vec<String>,
    /// Lines removed, without the leading `-`.
    pub removed: Vec<String>,
}

impl FileChange {
    /// A file with no line-level detail yet.
    pub fn new(path: impl Into<String>, kind: ChangeKind) -> Self {
        Self {
            path: path.into(),
            kind,
            added: Vec::new(),
            removed: Vec::new(),
        }
    }
}

/// A set of file changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    /// One entry per changed file.
    pub files: Vec<FileChange>,
}

impl Diff {
    /// Total lines added across all files.
    pub fn lines_added(&self) -> usize {
        self.files.iter().map(|f| f.added.len()).sum()
    }

    /// Total lines removed across all files.
    pub fn lines_removed(&self) -> usize {
        self.files.iter().map(|f| f.removed.len()).sum()
    }
}

/// Parse unified diff text as produced by `git diff --no-color -U0`.
///
/// Tolerant: unrecognised lines are skipped, and a truncated diff yields the
/// files seen so far.
pub fn parse_unified_diff(text: &str) -> Diff {
    let mut files: Vec<FileChange> = Vec::new();
    let mut current: Option<FileChange> = None;
    let mut in_hunk = false;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.extend(current.take());
            current = Some(FileChange::new(header_path(rest), ChangeKind::Modified));
            in_hunk = false;
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk && line.starts_with('+') {
            file.added.push(line[1..].to_string());
        } else if in_hunk && line.starts_with('-') {
            file.removed.push(line[1..].to_string());
        } else if !in_hunk {
            if line.starts_with("new file mode") {
                file.kind = ChangeKind::Added;
            } else if line.starts_with("deleted file mode") {
                file.kind = ChangeKind::Deleted;
            } else if let Some(to) = line.strip_prefix("rename to ") {
                file.kind = ChangeKind::Renamed;
                file.path = unquote(to);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                // `+++ /dev/null` (a deleted file) has no `b/` prefix and is skipped.
                if let Some(path) = unquote(path).strip_prefix("b/") {
                    file.path = path.to_string();
                }
            } else if let Some(path) = line.strip_prefix("--- ") {
                // Only a deleted file has no `+++ b/` line to take the path from.
                if file.kind == ChangeKind::Deleted {
                    if let Some(path) = unquote(path).strip_prefix("a/") {
                        file.path = path.to_string();
                    }
                }
            }
        }
    }
    files.extend(current);
    Diff { files }
}

/// Best-effort path from `a/<path> b/<path>` in a `diff --git` header.
fn header_path(rest: &str) -> String {
    let rest = rest.trim();
    if let Some(i) = rest.rfind(" \"b/") {
        let quoted = unquote(&rest[i + 1..]);
        return quoted.strip_prefix("b/").unwrap_or(&quoted).to_string();
    }
    match rest.rfind(" b/") {
        Some(i) => rest[i + 3..].to_string(),
        None => unquote(rest),
    }
}

/// Strip the quotes git puts around unusual paths.
fn unquote(path: &str) -> String {
    let path = path.trim();
    path.strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-let a = 1;
+let a = 2;
@@ -10,0 +11,1 @@
+new line
";

    #[test]
    fn parses_a_modified_file() {
        let diff = parse_unified_diff(MODIFIED);
        assert_eq!(diff.files.len(), 1);
        let f = &diff.files[0];
        assert_eq!(f.path, "src/lib.rs");
        assert_eq!(f.kind, ChangeKind::Modified);
        assert_eq!(f.added, ["let a = 2;", "new line"]);
        assert_eq!(f.removed, ["let a = 1;"]);
        assert_eq!((diff.lines_added(), diff.lines_removed()), (2, 1));
    }

    #[test]
    fn parses_added_and_deleted_files() {
        let text = "\
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
diff --git a/tests/old_test.py b/tests/old_test.py
deleted file mode 100644
--- a/tests/old_test.py
+++ /dev/null
@@ -1 +0,0 @@
-assert True
";
        let diff = parse_unified_diff(text);
        assert_eq!(diff.files.len(), 2);
        assert_eq!(diff.files[0].kind, ChangeKind::Added);
        assert_eq!(diff.files[0].path, "new.txt");
        assert_eq!(diff.files[1].kind, ChangeKind::Deleted);
        assert_eq!(diff.files[1].path, "tests/old_test.py");
        assert_eq!(diff.files[1].removed, ["assert True"]);
    }

    #[test]
    fn parses_renames_without_content_changes() {
        let text = "\
diff --git a/a.rs b/b.rs
similarity index 100%
rename from a.rs
rename to b.rs
";
        let diff = parse_unified_diff(text);
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].kind, ChangeKind::Renamed);
        assert_eq!(diff.files[0].path, "b.rs");
    }

    #[test]
    fn binary_files_have_a_path_and_no_lines() {
        let text = "\
diff --git a/img.png b/img.png
index 1..2 100644
Binary files a/img.png and b/img.png differ
";
        let diff = parse_unified_diff(text);
        assert_eq!(diff.files[0].path, "img.png");
        assert!(diff.files[0].added.is_empty());
    }

    #[test]
    fn content_lines_that_look_like_headers_are_content() {
        let text = "\
diff --git a/x.md b/x.md
--- a/x.md
+++ b/x.md
@@ -1 +1 @@
--- not a header
+++ also not a header
";
        let diff = parse_unified_diff(text);
        assert_eq!(diff.files[0].removed, ["-- not a header"]);
        assert_eq!(diff.files[0].added, ["++ also not a header"]);
        assert_eq!(diff.files[0].path, "x.md");
    }

    #[test]
    fn quoted_paths_are_unquoted() {
        let text = "diff --git \"a/with space.txt\" \"b/with space.txt\"\n";
        let diff = parse_unified_diff(text);
        assert_eq!(diff.files[0].path, "with space.txt");
    }

    #[test]
    fn empty_and_garbage_input_yield_no_files() {
        assert!(parse_unified_diff("").files.is_empty());
        assert!(parse_unified_diff("nonsense\nmore nonsense")
            .files
            .is_empty());
    }
}
