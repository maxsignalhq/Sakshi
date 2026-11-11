//! The trust engine: pure functions over events and a diff that produce
//! findings with a severity and a plain-English reason, and a badge.
//!
//! Nothing here reads files or runs commands. Callers supply the events a
//! session produced and, if available, the diff of what changed.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::diff::{ChangeKind, Diff};
use crate::event::{Event, EventKind};
use crate::redact::looks_like_secret;

/// How worrying a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Worth knowing; does not change the badge.
    Info,
    /// Look at this; turns the badge yellow.
    Warn,
    /// Serious; turns the badge red.
    Alert,
}

impl Severity {
    /// The name used in logs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Alert => "alert",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A string was not a valid [`Severity`] or [`Badge`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown trust value: {0:?}")]
pub struct ParseTrustError(String);

impl FromStr for Severity {
    type Err = ParseTrustError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "info" => Ok(Severity::Info),
            "warn" => Ok(Severity::Warn),
            "alert" => Ok(Severity::Alert),
            other => Err(ParseTrustError(other.to_string())),
        }
    }
}

/// The at-a-glance verdict on a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Badge {
    /// Nothing looks off.
    Green,
    /// Something deserves a look.
    Yellow,
    /// Something looks seriously wrong.
    Red,
}

impl Badge {
    /// The name used in logs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Badge::Green => "green",
            Badge::Yellow => "yellow",
            Badge::Red => "red",
        }
    }

    /// The badge for a set of findings: the worst severity decides.
    pub fn from_findings(findings: &[Finding]) -> Badge {
        match findings.iter().map(|f| f.severity).max() {
            Some(Severity::Alert) => Badge::Red,
            Some(Severity::Warn) => Badge::Yellow,
            _ => Badge::Green,
        }
    }
}

impl fmt::Display for Badge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Badge {
    type Err = ParseTrustError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "green" => Ok(Badge::Green),
            "yellow" => Ok(Badge::Yellow),
            "red" => Ok(Badge::Red),
            other => Err(ParseTrustError(other.to_string())),
        }
    }
}

/// One thing the trust engine noticed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable identifier of the rule that produced it, e.g. `tests-deleted`.
    pub rule: String,
    /// How worrying it is.
    pub severity: Severity,
    /// What happened, in plain English.
    pub reason: String,
}

impl Finding {
    fn new(rule: &str, severity: Severity, reason: impl Into<String>) -> Self {
        Self {
            rule: rule.to_string(),
            severity,
            reason: reason.into(),
        }
    }
}

/// The question a finding answers. Every rule belongs to exactly one, so a
/// list of findings reads as a handful of answers instead of a list of rules.
///
/// The concern is derived from the rule name, so it never appears on the wire
/// or in the activity log and old logs group the same way as new ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Concern {
    /// Did the agent make a check pass without doing the work?
    FakedSuccess,
    /// Did it do something dangerous?
    RiskyAction,
    /// Is the work verified?
    Unverified,
    /// What deserves a human look?
    Review,
    /// What could Sakshi not see?
    BlindSpot,
}

impl Concern {
    /// Every concern, in the order they are shown.
    pub const ALL: [Concern; 5] = [
        Concern::FakedSuccess,
        Concern::RiskyAction,
        Concern::Unverified,
        Concern::Review,
        Concern::BlindSpot,
    ];

    /// The stable name used in JSON reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Concern::FakedSuccess => "faked-success",
            Concern::RiskyAction => "risky-action",
            Concern::Unverified => "unverified",
            Concern::Review => "review",
            Concern::BlindSpot => "blind-spot",
        }
    }

    /// The heading shown to people.
    pub fn title(self) -> &'static str {
        match self {
            Concern::FakedSuccess => "Faked success",
            Concern::RiskyAction => "Risky actions",
            Concern::Unverified => "Not verified",
            Concern::Review => "Worth reviewing",
            Concern::BlindSpot => "Blind spots",
        }
    }

    /// The concern of a built-in rule, or `None` for a name Sakshi does not know.
    pub fn of_known_rule(rule: &str) -> Option<Concern> {
        Some(match rule {
            "tests-deleted"
            | "tests-skipped"
            | "assertions-weakened"
            | "trivial-assertion"
            | "coverage-lowered"
            | "checks-suppressed" => Concern::FakedSuccess,
            "destructive-command" | "sensitive-file" | "secret-added" | "outside-project" => {
                Concern::RiskyAction
            }
            "tests-failing" | "repeated-failure" | "unverified-changes" => Concern::Unverified,
            "large-change" | "lockfile-changed" | "manifest-changed" | "session-overlap" => {
                Concern::Review
            }
            "hooks-silent" => Concern::BlindSpot,
            _ => return None,
        })
    }

    /// The concern of any rule name. A rule this build does not know (a log
    /// written by a newer Sakshi) is something to review.
    pub fn of_rule(rule: &str) -> Concern {
        Concern::of_known_rule(rule).unwrap_or(Concern::Review)
    }
}

impl fmt::Display for Concern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Finding {
    /// The question this finding answers.
    pub fn concern(&self) -> Concern {
        Concern::of_rule(&self.rule)
    }
}

/// The verdict: a badge and the findings behind it, worst first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustReport {
    /// Overall verdict.
    pub badge: Badge,
    /// Findings, most severe first.
    pub findings: Vec<Finding>,
}

impl TrustReport {
    /// A report with no findings.
    pub fn clean() -> Self {
        Self {
            badge: Badge::Green,
            findings: Vec::new(),
        }
    }

    /// The findings grouped by the question they answer, in [`Concern::ALL`]
    /// order, most severe first within a group. Empty groups are left out.
    pub fn by_concern(&self) -> Vec<(Concern, Vec<&Finding>)> {
        Concern::ALL
            .into_iter()
            .filter_map(|concern| {
                let group: Vec<&Finding> = self
                    .findings
                    .iter()
                    .filter(|f| f.concern() == concern)
                    .collect();
                (!group.is_empty()).then_some((concern, group))
            })
            .collect()
    }

    /// The finding that decided the badge: the first of the most severe ones
    /// that is not just a note. `None` for a green verdict.
    pub fn worst(&self) -> Option<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity > Severity::Info)
            .max_by_key(|f| (f.severity, std::cmp::Reverse(f.concern())))
    }
}

/// A path pattern the user wants flagged, with how serious touching it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRule {
    /// Glob over `/`-separated paths: `*` and `?` within a segment, `**` across
    /// segments. Patterns not starting with `/` match anywhere in the path.
    pub pattern: String,
    /// Severity of a finding when a matching path is touched.
    pub severity: Severity,
}

/// User adjustments to the built-in rules. The default changes nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustConfig {
    /// Extra paths to treat as sensitive.
    pub sensitive_paths: Vec<PathRule>,
    /// Paths never reported as sensitive, not even by the built-in rules.
    pub ignore_paths: Vec<String>,
}

/// What Sakshi saw about a session that is not in its event log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observations {
    /// The agent is expected to report through hooks (Claude Code and Codex are).
    pub hooks_expected: bool,
    /// Bytes the session's terminal has printed so far.
    pub output_bytes: u64,
    /// Seconds since the session started.
    pub running_secs: u64,
    /// The session's working directory. Paths under it are matched relative
    /// to it, so a project folder called `payments` does not flag every file.
    pub root: Option<String>,
    /// Files this session edited that another running session edited too.
    pub shared_edits: Vec<SharedEdit>,
}

/// A file two running sessions have both edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedEdit {
    /// The file, as an absolute `/`-separated path.
    pub path: String,
    /// Name of the other session that edited it.
    pub session: String,
}

/// The absolute, `/`-separated form of an edited path, or `None` for a
/// relative one (which cannot be compared between sessions).
pub fn edited_path(path: &str) -> Option<String> {
    let path = normalize(path);
    is_absolute(&path).then_some(path)
}

/// The files in `mine` that any of the named `peers` also edited.
pub fn shared_edits(
    mine: &BTreeSet<String>,
    peers: &[(&str, &BTreeSet<String>)],
) -> Vec<SharedEdit> {
    let mut out = Vec::new();
    for path in mine {
        if let Some((name, _)) = peers.iter().find(|(_, theirs)| theirs.contains(path)) {
            out.push(SharedEdit {
                path: path.clone(),
                session: (*name).to_string(),
            });
        }
    }
    out
}

/// Seconds of activity without a hook report before `hooks-silent` fires.
pub const HOOK_SILENCE_SECS: u64 = 60;
/// Terminal output, in bytes, that counts as "visibly doing something".
pub const HOOK_SILENCE_OUTPUT_BYTES: u64 = 4096;

/// Evaluate every rule over a session's `events` and, if known, its `diff`,
/// with the default configuration and no extra observations.
pub fn evaluate(events: &[Event], diff: Option<&Diff>) -> TrustReport {
    evaluate_with(
        events,
        diff,
        &TrustConfig::default(),
        &Observations::default(),
    )
}

/// Evaluate every rule, honouring the user's `config` and what Sakshi
/// `observed` about the session.
pub fn evaluate_with(
    events: &[Event],
    diff: Option<&Diff>,
    config: &TrustConfig,
    observed: &Observations,
) -> TrustReport {
    let mut findings = Vec::new();
    if let Some(diff) = diff {
        findings.extend(tests_deleted(diff));
        findings.extend(tests_skipped(diff));
        findings.extend(assertions_weakened(diff));
        findings.extend(trivial_assertions(diff));
        findings.extend(coverage_lowered(diff));
        findings.extend(checks_suppressed(diff));
        findings.extend(secrets_added(diff));
        findings.extend(large_change(diff));
        findings.extend(lockfiles_changed(diff));
        findings.extend(manifests_changed(diff));
    }
    findings.extend(sensitive_files(
        events,
        diff,
        config,
        observed.root.as_deref(),
    ));
    findings.extend(outside_project(events, config, observed.root.as_deref()));
    if !observed.shared_edits.is_empty() {
        let mut list = observed
            .shared_edits
            .iter()
            .take(3)
            .map(|e| format!("{} (also edited by {})", e.path, e.session))
            .collect::<Vec<_>>()
            .join(", ");
        if observed.shared_edits.len() > 3 {
            list.push_str(&format!(" and {} more", observed.shared_edits.len() - 3));
        }
        findings.push(Finding::new(
            "session-overlap",
            Severity::Warn,
            format!("another running session edited the same files, so their changes may collide: {list}"),
        ));
    }
    findings.extend(destructive_commands(events));
    // A streak of identical failures says more than the single latest failure.
    let stuck = repeated_failure(events);
    if stuck.is_empty() {
        findings.extend(tests_failing(events));
    } else {
        findings.extend(stuck);
    }
    findings.extend(unverified_changes(events));
    if hooks_silent(events, observed) {
        findings.push(Finding::new(
            "hooks-silent",
            Severity::Warn,
            "the agent is busy but its hooks have not reported anything, so live \
             state and some findings may be missing (for Codex, run `sakshi setup \
             codex` and approve the hooks once)",
        ));
    }
    // Stable sort: equal severities keep rule order.
    findings.sort_by_key(|f| std::cmp::Reverse(f.severity));
    TrustReport {
        badge: Badge::from_findings(&findings),
        findings,
    }
}

// ---------------------------------------------------------------- paths

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

/// Whether `path` looks like a test file.
pub fn is_test_path(path: &str) -> bool {
    let path = normalize(path);
    let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some(file) = segments.pop() else {
        return false;
    };
    const TEST_DIRS: &[&str] = &["test", "tests", "__tests__", "spec", "specs", "e2e"];
    if segments
        .iter()
        .any(|s| TEST_DIRS.contains(&s.to_ascii_lowercase().as_str()))
    {
        return true;
    }
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let lower = stem.to_ascii_lowercase();
    lower.starts_with("test_")
        || lower.starts_with("test-")
        || ["_test", "_tests", ".test", ".spec", "_spec", "-test", "-spec"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
        // CamelCase `FooTest` / `FooTests`, without matching words like `latest`.
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
}

fn sensitivity(path: &str) -> Option<(Severity, &'static str)> {
    let lower = format!(
        "/{}",
        normalize(path).trim_start_matches('/').to_ascii_lowercase()
    );
    let name = lower.rsplit('/').next().unwrap_or("");
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");

    let is_env = name == ".env"
        || (name.starts_with(".env.")
            && !["example", "sample", "template", "dist"]
                .iter()
                .any(|s| name.ends_with(s)));
    let is_key_file = !name.ends_with(".pub")
        && (["pem", "key", "p12", "pfx"].contains(&ext)
            || ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"].contains(&name));
    let is_credential = [".npmrc", ".pypirc", ".netrc", ".pgpass"].contains(&name)
        || name == "credentials"
        || name.starts_with("credentials.")
        || name.starts_with("secrets.");
    let in_secret_dir = ["/.ssh/", "/.aws/", "/.gnupg/", "/.kube/"]
        .iter()
        .any(|d| lower.contains(d));

    if is_env || is_key_file || is_credential || in_secret_dir {
        return Some((Severity::Alert, "secrets or credentials"));
    }
    if lower.contains("/.git/") {
        return Some((Severity::Alert, "git internals"));
    }
    let is_automation = lower.contains("/.github/workflows/")
        || lower.contains("/.circleci/")
        || lower.contains("/.husky/")
        || [
            ".gitlab-ci.yml",
            "jenkinsfile",
            "azure-pipelines.yml",
            "codeowners",
            ".pre-commit-config.yaml",
        ]
        .contains(&name);
    if is_automation {
        return Some((Severity::Warn, "CI or automation config"));
    }
    None
}

/// Whether `path` is documentation, where rule keywords are just words.
fn is_doc_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".markdown", ".rst", ".txt", ".adoc"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Lower-case words of `s`, split at punctuation and camelCase boundaries:
/// `OAuth2Client/login.rs` gives `oauth2`, `client`, `login`, `rs`.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current).to_ascii_lowercase());
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower && !current.is_empty() {
            out.push(std::mem::take(&mut current).to_ascii_lowercase());
        }
        current.push(c);
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
    }
    if !current.is_empty() {
        out.push(current.to_ascii_lowercase());
    }
    out
}

/// Code that guards money, identity or data: worth a look when an agent
/// changes it. Test and doc files are left to the test-integrity rules.
fn business_sensitivity(rel: &str) -> Option<&'static str> {
    const AUTH: &[&str] = &[
        "auth",
        "authn",
        "authz",
        "oauth",
        "oauth2",
        "authentication",
        "authorization",
        "authorisation",
        "login",
        "sso",
        "saml",
        "oidc",
        "jwt",
        "rbac",
    ];
    const PAYMENTS: &[&str] = &[
        "payment", "payments", "billing", "invoice", "invoices", "checkout", "ledger", "refund",
        "refunds", "payout", "payouts", "stripe",
    ];
    const MIGRATIONS: &[&str] = &[
        "migration",
        "migrations",
        "migrate",
        "alembic",
        "flyway",
        "liquibase",
    ];
    if is_test_path(rel) || is_doc_path(rel) {
        return None;
    }
    let words = words(rel);
    let any = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    if any(AUTH) {
        Some("authentication code")
    } else if any(PAYMENTS) {
        Some("payments code")
    } else if any(MIGRATIONS) {
        Some("a database migration")
    } else {
        None
    }
}

/// `path` relative to `root` when it lies under it; otherwise `path` itself.
/// Comparison ignores case and slash direction.
fn relative_to(path: &str, root: Option<&str>) -> String {
    let path = normalize(path);
    if let Some(root) = root {
        let root = normalize(root);
        let root = root.trim_end_matches('/');
        // `get` yields None when the offset is not a char boundary, which
        // means a multi-byte character straddles it and the path cannot be
        // under the root.
        if !root.is_empty() {
            if let (Some(head), Some(rest)) = (path.get(..root.len()), path.get(root.len()..)) {
                if head.eq_ignore_ascii_case(root) && rest.starts_with('/') {
                    return rest.trim_start_matches('/').to_string();
                }
            }
        }
    }
    path
}

/// Whether a `/`-separated `path` matches `pattern` (case-insensitive).
/// `*` and `?` match within a segment and `**` matches any number of
/// segments. A pattern that does not start with `/` may match at any depth.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = normalize(pattern).to_ascii_lowercase();
    let path = normalize(path).to_ascii_lowercase();
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if pat.is_empty() {
        return false;
    }
    if pattern.starts_with('/') {
        match_segments(&pat, &segs)
    } else {
        (0..=segs.len()).any(|i| match_segments(&pat, &segs[i..]))
    }
}

fn match_segments(pat: &[&str], segs: &[&str]) -> bool {
    match pat.split_first() {
        None => segs.is_empty(),
        Some((&"**", rest)) => (0..=segs.len()).any(|i| match_segments(rest, &segs[i..])),
        Some((first, rest)) => segs
            .split_first()
            .is_some_and(|(head, tail)| match_segment(first, head) && match_segments(rest, tail)),
    }
}

fn match_segment(pat: &str, seg: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len()).any(|i| go(rest, &s[i..])),
            Some(('?', rest)) => !s.is_empty() && go(rest, &s[1..]),
            Some((c, rest)) => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = seg.chars().collect();
    go(&p, &s)
}

/// Every reason to treat `path` as sensitive, worst first: the user's
/// patterns, the built-in secret and CI rules, and business-critical code.
fn classify_path(
    path: &str,
    root: Option<&str>,
    config: &TrustConfig,
) -> Option<(Severity, String)> {
    let rel = relative_to(path, root);
    let ignored = config
        .ignore_paths
        .iter()
        .any(|p| glob_match(p, &rel) || glob_match(p, path));
    if ignored {
        return None;
    }
    let mut found: Vec<(Severity, String)> = Vec::new();
    if let Some(rule) = config
        .sensitive_paths
        .iter()
        .filter(|r| glob_match(&r.pattern, &rel) || glob_match(&r.pattern, path))
        .max_by_key(|r| r.severity)
    {
        found.push((
            rule.severity,
            format!("a path you marked sensitive (`{}`)", rule.pattern),
        ));
    }
    if let Some((severity, what)) = sensitivity(path) {
        found.push((severity, what.to_string()));
    }
    if let Some(what) = business_sensitivity(&rel) {
        found.push((Severity::Warn, what.to_string()));
    }
    // The first of the worst: user rules win ties, so their wording shows.
    let worst = found.iter().map(|(s, _)| *s).max()?;
    found.into_iter().find(|(s, _)| *s == worst)
}

// ---------------------------------------------------------------- diff rules

fn tests_deleted(diff: &Diff) -> Vec<Finding> {
    diff.files
        .iter()
        .filter(|f| f.kind == ChangeKind::Deleted && is_test_path(&f.path))
        .map(|f| {
            Finding::new(
                "tests-deleted",
                Severity::Alert,
                format!("deleted test file {}", f.path),
            )
        })
        .collect()
}

/// Markers that switch tests off or narrow a run.
fn skip_marker(line: &str, rust_file: bool) -> Option<&'static str> {
    const MARKERS: &[&str] = &[
        "@pytest.mark.skip",
        "pytest.skip(",
        "unittest.skip",
        ".skip(",
        "xit(",
        "xdescribe(",
        "xtest(",
        "it.todo(",
        "test.todo(",
        ".only(",
        "t.Skip(",
        "t.Skipf(",
        "t.SkipNow(",
        "@Disabled",
        "@Ignore",
        "[Ignore]",
        "Skip =",
    ];
    if rust_file && line.contains("#[ignore") {
        return Some("#[ignore]");
    }
    MARKERS.iter().copied().find(|m| line.contains(m))
}

fn tests_skipped(diff: &Diff) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in &diff.files {
        let rust = file.path.ends_with(".rs");
        if !(is_test_path(&file.path) || rust) {
            continue;
        }
        let markers: Vec<&str> = file
            .added
            .iter()
            .filter_map(|l| skip_marker(l, rust))
            // Outside test files only Rust's inline `#[ignore]` counts.
            .filter(|m| is_test_path(&file.path) || *m == "#[ignore]")
            .collect();
        if let Some(first) = markers.first() {
            out.push(Finding::new(
                "tests-skipped",
                Severity::Warn,
                format!(
                    "added {} test skip/focus marker(s) such as `{first}` in {}",
                    markers.len(),
                    file.path
                ),
            ));
        }
    }
    out
}

fn is_assertion(line: &str) -> bool {
    ["assert", "expect(", ".should", "require."]
        .iter()
        .any(|m| line.contains(m))
}

fn assertions_weakened(diff: &Diff) -> Vec<Finding> {
    let (mut removed, mut added) = (0usize, 0usize);
    // Deleted test files have their own rule; counting them here would
    // report the same problem twice.
    let edited_tests = diff
        .files
        .iter()
        .filter(|f| f.kind != ChangeKind::Deleted && is_test_path(&f.path));
    for file in edited_tests {
        removed += file.removed.iter().filter(|l| is_assertion(l)).count();
        added += file.added.iter().filter(|l| is_assertion(l)).count();
    }
    if removed > added {
        vec![Finding::new(
            "assertions-weakened",
            Severity::Warn,
            format!(
                "tests lost {} assertion(s) net ({removed} removed, {added} added)",
                removed - added
            ),
        )]
    } else {
        Vec::new()
    }
}

fn trivial_assertions(diff: &Diff) -> Vec<Finding> {
    const TRIVIAL: &[&str] = &[
        "assert!(true)",
        "assert_eq!(true, true)",
        "assert True",
        "assert(true)",
        "assertTrue(true)",
        "expect(true).toBe(true)",
        "expect(true).toEqual(true)",
        "assert.ok(true)",
    ];
    diff.files
        .iter()
        .filter(|f| is_test_path(&f.path))
        .filter(|f| {
            f.added
                .iter()
                .any(|l| TRIVIAL.iter().any(|t| l.contains(t)))
        })
        .map(|f| {
            Finding::new(
                "trivial-assertion",
                Severity::Warn,
                format!("added an assertion that is always true in {}", f.path),
            )
        })
        .collect()
}

/// Numbers in `line`, and the line with each number replaced by `#`, so
/// `fail_under = 90` and `fail_under = 40` share the template `fail_under = #`.
fn numbers_and_template(line: &str) -> (Vec<f64>, String) {
    let mut numbers = Vec::new();
    let mut template = String::new();
    let mut digits = String::new();
    let flush = |digits: &mut String, numbers: &mut Vec<f64>, template: &mut String| {
        if digits.is_empty() {
            return;
        }
        let text = digits.trim_end_matches('.');
        if let Ok(n) = text.parse::<f64>() {
            numbers.push(n);
        }
        template.push('#');
        if digits.ends_with('.') {
            template.push('.');
        }
        digits.clear();
    };
    for c in line.trim().trim_end_matches(',').chars() {
        if c.is_ascii_digit() || (c == '.' && !digits.is_empty() && !digits.contains('.')) {
            digits.push(c);
        } else {
            flush(&mut digits, &mut numbers, &mut template);
            template.push(c);
        }
    }
    flush(&mut digits, &mut numbers, &mut template);
    (numbers, template)
}

/// Whether a changed line in `path` could hold a coverage threshold.
fn is_coverage_line(path: &str, line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const HINTS: &[&str] = &[
        "coverage",
        "--cov",
        "cov-fail",
        "cov_fail",
        "fail_under",
        "fail-under",
        "failunder",
    ];
    if HINTS.iter().any(|h| lower.contains(h)) {
        return true;
    }
    // Jest, nyc, vitest and codecov nest the numbers under bare metric keys.
    let name = normalize(path)
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    const CONFIGS: &[&str] = &[
        "jest.config",
        "vitest.config",
        ".nycrc",
        "codecov",
        ".coveragerc",
        "karma.conf",
        "tarpaulin",
        "package.json",
    ];
    const KEYS: &[&str] = &[
        "branches",
        "functions",
        "lines",
        "statements",
        "threshold",
        "target",
    ];
    let key = lower
        .trim()
        .trim_start_matches(['"', '\''])
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .next()
        .unwrap_or("");
    CONFIGS.iter().any(|c| name.contains(c)) && KEYS.contains(&key)
}

fn coverage_lowered(diff: &Diff) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in &diff.files {
        if is_doc_path(&file.path) {
            continue;
        }
        let mut added: Vec<(Vec<f64>, String, &str)> = file
            .added
            .iter()
            .filter(|l| is_coverage_line(&file.path, l))
            .map(|l| {
                let (n, t) = numbers_and_template(l);
                (n, t, l.as_str())
            })
            .collect();
        for old in file
            .removed
            .iter()
            .filter(|l| is_coverage_line(&file.path, l))
        {
            let (old_numbers, template) = numbers_and_template(old);
            if old_numbers.is_empty() {
                continue;
            }
            let Some(at) = added
                .iter()
                .position(|(n, t, _)| *t == template && n.len() == old_numbers.len())
            else {
                continue;
            };
            let (new_numbers, _, new) = added.remove(at);
            let pairs = old_numbers.iter().zip(&new_numbers);
            let lowered = pairs.clone().any(|(o, n)| n < o) && !pairs.clone().any(|(o, n)| n > o);
            if lowered {
                out.push(Finding::new(
                    "coverage-lowered",
                    Severity::Alert,
                    format!(
                        "lowered a coverage threshold in {}: `{}` to `{}`",
                        file.path,
                        shorten(old),
                        shorten(new)
                    ),
                ));
            }
        }
    }
    out
}

/// Comments and attributes that silence a linter, type checker or
/// coverage tool instead of fixing what it found.
fn suppression_marker(line: &str) -> Option<&'static str> {
    const MARKERS: &[&str] = &[
        "eslint-disable",
        "@ts-nocheck",
        "@ts-ignore",
        "@ts-expect-error",
        "biome-ignore",
        "deno-lint-ignore",
        "# noqa",
        "#noqa",
        "# type: ignore",
        "pylint: disable",
        "#[allow(",
        "#![allow(",
        "@SuppressWarnings",
        "//nolint",
        "// nolint",
        "rubocop:disable",
        "#pragma warning disable",
        "NOSONAR",
        "istanbul ignore",
        "c8 ignore",
        "@phpstan-ignore",
        "swiftlint:disable",
    ];
    MARKERS.iter().copied().find(|m| line.contains(m))
}

fn checks_suppressed(diff: &Diff) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in diff.files.iter().filter(|f| !is_doc_path(&f.path)) {
        let markers: Vec<&str> = file
            .added
            .iter()
            .filter_map(|l| suppression_marker(l))
            .collect();
        if let Some(first) = markers.first() {
            out.push(Finding::new(
                "checks-suppressed",
                Severity::Warn,
                format!(
                    "added {} lint/type/coverage suppression(s) such as `{first}` in {}",
                    markers.len(),
                    file.path
                ),
            ));
        }
    }
    out
}

/// Lowercase file names of dependency lockfiles.
const LOCKFILES: &[&str] = &[
    "cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
    "poetry.lock",
    "pipfile.lock",
    "uv.lock",
    "gemfile.lock",
    "go.sum",
    "composer.lock",
    "packages.lock.json",
    "podfile.lock",
    "pubspec.lock",
    "mix.lock",
];

/// Files changed (lockfiles excluded) at which a session counts as a big one.
const LARGE_FILES: usize = 25;
/// Lines added plus removed at which a session counts as a big one.
const LARGE_LINES: usize = 1000;
/// Twice over, and the finding becomes a warning.
const HUGE_FACTOR: usize = 3;

fn large_change(diff: &Diff) -> Vec<Finding> {
    let counted: Vec<_> = diff
        .files
        .iter()
        .filter(|f| {
            let name = normalize(&f.path)
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            !LOCKFILES.contains(&name.as_str())
        })
        .collect();
    let files = counted.len();
    let lines: usize = counted
        .iter()
        .map(|f| f.added.len() + f.removed.len())
        .sum();
    if files < LARGE_FILES && lines < LARGE_LINES {
        return Vec::new();
    }
    let huge = files >= LARGE_FILES * HUGE_FACTOR || lines >= LARGE_LINES * HUGE_FACTOR;
    vec![Finding::new(
        "large-change",
        if huge { Severity::Warn } else { Severity::Info },
        format!(
            "a big change: {files} files and about {lines} lines; plan extra time to review it"
        ),
    )]
}

fn secrets_added(diff: &Diff) -> Vec<Finding> {
    let files: Vec<&str> = diff
        .files
        .iter()
        .filter(|f| f.added.iter().any(|l| looks_like_secret(l)))
        .map(|f| f.path.as_str())
        .collect();
    if files.is_empty() {
        return Vec::new();
    }
    let mut list = files.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    if files.len() > 3 {
        list.push_str(&format!(" and {} more", files.len() - 3));
    }
    // Name the files, never the secret.
    vec![Finding::new(
        "secret-added",
        Severity::Alert,
        format!("added lines look like a credential or private key ({list}); do not commit it"),
    )]
}

fn lockfiles_changed(diff: &Diff) -> Vec<Finding> {
    changed_files(
        diff,
        |name| LOCKFILES.contains(&name),
        "lockfile-changed",
        "dependency lockfile changed",
    )
}

fn manifests_changed(diff: &Diff) -> Vec<Finding> {
    const MANIFESTS: &[&str] = &[
        "cargo.toml",
        "package.json",
        "pyproject.toml",
        "pipfile",
        "go.mod",
        "gemfile",
        "composer.json",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "mix.exs",
        "pubspec.yaml",
    ];
    changed_files(
        diff,
        |name| {
            MANIFESTS.contains(&name)
                || (name.starts_with("requirements") && name.ends_with(".txt"))
        },
        "manifest-changed",
        "dependency manifest changed",
    )
}

/// An info finding listing the changed files whose lowercase name satisfies `is_match`.
fn changed_files(
    diff: &Diff,
    is_match: impl Fn(&str) -> bool,
    rule: &str,
    what: &str,
) -> Vec<Finding> {
    let changed: Vec<&str> = diff
        .files
        .iter()
        .filter(|f| {
            let name = normalize(&f.path)
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            is_match(&name)
        })
        .map(|f| f.path.as_str())
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }
    let mut list = changed
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if changed.len() > 3 {
        list.push_str(&format!(" and {} more", changed.len() - 3));
    }
    vec![Finding::new(
        rule,
        Severity::Info,
        format!("{what} ({list}); check what was added or upgraded"),
    )]
}

// ---------------------------------------------------------------- event rules

/// Whether the agent has been visibly busy for a while without a single hook
/// report: its hooks are probably not reaching Sakshi.
pub fn hooks_silent(events: &[Event], observed: &Observations) -> bool {
    observed.hooks_expected
        && observed.running_secs >= HOOK_SILENCE_SECS
        && observed.output_bytes >= HOOK_SILENCE_OUTPUT_BYTES
        && !events
            .iter()
            .any(|e| !matches!(e.kind, EventKind::Session { .. } | EventKind::Trust { .. }))
}

fn sensitive_files(
    events: &[Event],
    diff: Option<&Diff>,
    config: &TrustConfig,
    root: Option<&str>,
) -> Vec<Finding> {
    let mut paths: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::FileEdit { path } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    if let Some(diff) = diff {
        paths.extend(diff.files.iter().map(|f| f.path.as_str()));
    }
    let mut out: Vec<Finding> = Vec::new();
    for path in paths {
        let Some((severity, what)) = classify_path(path, root, config) else {
            continue;
        };
        // Name the file relative to the project so an absolute path from a
        // hook and a relative one from git are reported once.
        let reason = format!("touched {what}: {}", relative_to(path, root));
        if !out.iter().any(|f| f.reason == reason) {
            out.push(Finding::new("sensitive-file", severity, reason));
        }
    }
    out
}

/// Whether `path` (already normalised) is absolute on Unix or Windows.
fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/')
}

/// Places an agent routinely writes outside the project: scratch space and
/// its own notes and plans. Matched in lowercase.
fn routine_outside_path(lower: &str) -> bool {
    const TEMP: &[&str] = &["/tmp/", "/var/folders/", "/appdata/local/temp/"];
    const AGENT_STATE: &[&str] = &[
        "/.claude/projects/",
        "/.claude/plans/",
        "/.claude/todos/",
        "/.codex/memories/",
    ];
    TEMP.iter().chain(AGENT_STATE).any(|m| lower.contains(m))
}

fn outside_project(events: &[Event], config: &TrustConfig, root: Option<&str>) -> Vec<Finding> {
    let Some(root) = root else {
        return Vec::new();
    };
    let mut outside: Vec<String> = Vec::new();
    for event in events {
        let EventKind::FileEdit { path } = &event.kind else {
            continue;
        };
        let path = normalize(path);
        if !is_absolute(&path)
            || relative_to(&path, Some(root)) != path
            || routine_outside_path(&path.to_ascii_lowercase())
            || config.ignore_paths.iter().any(|p| glob_match(p, &path))
            || outside.contains(&path)
        {
            continue;
        }
        outside.push(path);
    }
    if outside.is_empty() {
        return Vec::new();
    }
    let mut list = outside
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if outside.len() > 3 {
        list.push_str(&format!(" and {} more", outside.len() - 3));
    }
    vec![Finding::new(
        "outside-project",
        Severity::Warn,
        format!("edited files outside the session directory: {list}"),
    )]
}

fn shorten(command: &str) -> String {
    let one_line = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 80 {
        let cut: String = one_line.chars().take(80).collect();
        format!("{cut}…")
    } else {
        one_line
    }
}

fn destructive_commands(events: &[Event]) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    for event in events {
        let command = match &event.kind {
            EventKind::Command { command } => command,
            EventKind::TestRun { command, .. } => command,
            _ => continue,
        };
        for (severity, why) in analyze_command(command) {
            let reason = format!("ran `{}`: {why}", shorten(command));
            if !out.iter().any(|f| f.reason == reason) {
                out.push(Finding::new("destructive-command", severity, reason));
            }
        }
    }
    out
}

/// Targets of `rm -rf` that wipe far more than a build directory.
const CATASTROPHIC_TARGETS: &[&str] = &[
    "/", "/*", "~", "~/", "~/*", "$home", "${home}", "*", ".", "./", "./*", "..", "../", ".*",
    "c:\\", "c:/",
];

/// How deep `bash -c "bash -c ..."` nesting is followed.
const MAX_WRAPPER_DEPTH: usize = 3;

/// Programs that run the command that follows them.
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "nohup", "time", "command", "exec", "nice", "xargs", "setsid",
];

/// Shells that take a script after `-c`.
const POSIX_SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];

/// A command line split into pipelines of commands of words, honouring quotes.
/// `a && b | c` is two pipelines: `[a]` and `[b, c]`.
type Pipelines = Vec<Vec<Vec<String>>>;

/// Split a shell command line into words, honouring quotes, and into commands
/// at `;`, `&`, `&&`, `||`, `|` and newlines that are outside quotes. This is
/// a deliberately small subset of shell syntax: enough that a quoted `;` is not
/// a command boundary and `bash -c "..."` exposes its script.
fn parse_shell(line: &str) -> Pipelines {
    let mut pipelines: Pipelines = Vec::new();
    let mut pipeline: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;

    fn end_word(words: &mut Vec<String>, word: &mut String, in_word: &mut bool) {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    }
    fn end_command(pipeline: &mut Vec<Vec<String>>, words: &mut Vec<String>) {
        if !words.is_empty() {
            pipeline.push(std::mem::take(words));
        }
    }
    fn end_pipeline(pipelines: &mut Pipelines, pipeline: &mut Vec<Vec<String>>) {
        if !pipeline.is_empty() {
            pipelines.push(std::mem::take(pipeline));
        }
    }

    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' && matches!(chars.peek(), Some('"' | '\\')) {
                    word.extend(chars.next());
                } else {
                    word.push(c);
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    in_word = true;
                }
                c if c.is_whitespace() && c != '\n' => {
                    end_word(&mut words, &mut word, &mut in_word);
                }
                '|' => {
                    end_word(&mut words, &mut word, &mut in_word);
                    end_command(&mut pipeline, &mut words);
                    // `||` separates pipelines; a lone `|` joins commands.
                    if chars.peek() == Some(&'|') {
                        chars.next();
                        end_pipeline(&mut pipelines, &mut pipeline);
                    }
                }
                ';' | '&' | '\n' => {
                    end_word(&mut words, &mut word, &mut in_word);
                    end_command(&mut pipeline, &mut words);
                    end_pipeline(&mut pipelines, &mut pipeline);
                    if c == '&' && chars.peek() == Some(&'&') {
                        chars.next();
                    }
                }
                _ => {
                    word.push(c);
                    in_word = true;
                }
            },
        }
    }
    end_word(&mut words, &mut word, &mut in_word);
    end_command(&mut pipeline, &mut words);
    end_pipeline(&mut pipelines, &mut pipeline);
    pipelines
}

fn analyze_command(command: &str) -> Vec<(Severity, &'static str)> {
    let mut found = Vec::new();
    let lower = command.to_ascii_lowercase();

    for table_op in [
        "drop table",
        "drop database",
        "drop schema",
        "truncate table",
    ] {
        if lower.contains(table_op) {
            found.push((Severity::Alert, "destroys database data"));
            break;
        }
    }

    analyze_script(command, 0, &mut found);
    found
}

fn analyze_script(script: &str, depth: usize, found: &mut Vec<(Severity, &'static str)>) {
    for pipeline in parse_shell(script) {
        // `curl ... | sh` runs whatever the server sends.
        for pair in pipeline.windows(2) {
            let fetches = unwrap_wrappers(&pair[0])
                .first()
                .is_some_and(|w| matches!(program_name(w).as_str(), "curl" | "wget"));
            let runs = unwrap_wrappers(&pair[1])
                .first()
                .is_some_and(|w| POSIX_SHELLS.contains(&program_name(w).as_str()));
            if fetches && runs {
                found.push((Severity::Warn, "runs a downloaded script"));
            }
        }
        for words in &pipeline {
            found.extend(analyze_words(words, depth));
        }
    }
}

/// The lower-case program name of `word`: no directory, no `.exe`.
fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let lower = base.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Options of `sudo` that take a value.
const SUDO_VALUE_OPTIONS: &[&str] = &["-u", "-g", "-h", "-p", "-C", "-U", "-D", "-r", "-t"];

/// Words with leading `NAME=value` assignments and wrapper programs
/// (`sudo`, `env`, `nohup`, `xargs`, ...) removed, leaving the real command.
fn unwrap_wrappers(words: &[String]) -> &[String] {
    let mut rest = words;
    loop {
        match rest.first() {
            Some(w) if is_assignment(w) => rest = &rest[1..],
            Some(w) if WRAPPERS.contains(&program_name(w).as_str()) => {
                let is_sudo = program_name(w) == "sudo";
                rest = &rest[1..];
                // Skip the wrapper's own options, and the values of the ones
                // that take one (`sudo -u root`).
                while let Some(opt) = rest.first() {
                    if !opt.starts_with('-') && !is_assignment(opt) {
                        break;
                    }
                    let skip = if is_sudo && SUDO_VALUE_OPTIONS.contains(&opt.as_str()) {
                        2
                    } else {
                        1
                    };
                    rest = &rest[skip.min(rest.len())..];
                }
            }
            _ => return rest,
        }
    }
}

/// The script a shell invocation runs, if `program` is a shell given one.
fn embedded_script(program: &str, args: &[&str]) -> Option<String> {
    if POSIX_SHELLS.contains(&program) {
        let flag = args
            .iter()
            .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))?;
        return args.get(flag + 1).map(|s| (*s).to_string());
    }
    let after = |names: &[&str]| {
        args.iter()
            .position(|a| names.iter().any(|n| a.eq_ignore_ascii_case(n)))
            .map(|i| args[i + 1..].join(" "))
    };
    match program {
        "cmd" => after(&["/c", "/k"]),
        "powershell" | "pwsh" => after(&["-command", "-c"]),
        _ => None,
    }
}

fn analyze_words(words: &[String], depth: usize) -> Vec<(Severity, &'static str)> {
    let words = unwrap_wrappers(words);
    let Some(first) = words.first() else {
        return Vec::new();
    };
    let program = program_name(first);
    let args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
    let has = |flag: &str| args.contains(&flag);

    // A shell given a script to run: look inside it.
    if depth < MAX_WRAPPER_DEPTH {
        if let Some(script) = embedded_script(&program, &args) {
            let mut inner = Vec::new();
            analyze_script(&script, depth + 1, &mut inner);
            return inner;
        }
    }

    match program.as_str() {
        "git" => analyze_git(&args),
        "rm" => analyze_rm(&args),
        "remove-item" | "ri" => {
            let lower: Vec<String> = args.iter().map(|a| a.to_ascii_lowercase()).collect();
            let flag = |name: &str| lower.iter().any(|a| a == name);
            if flag("-recurse") && flag("-force") {
                vec![(Severity::Warn, "recursively force-deletes files")]
            } else {
                Vec::new()
            }
        }
        "rd" | "rmdir" | "del" | "erase" => {
            if args.iter().any(|a| a.eq_ignore_ascii_case("/s")) {
                vec![(Severity::Warn, "recursively deletes files")]
            } else {
                Vec::new()
            }
        }
        "dd" => {
            if args.iter().any(|a| a.starts_with("of=/dev/")) {
                vec![(Severity::Alert, "overwrites a raw device")]
            } else {
                Vec::new()
            }
        }
        p if p == "mkfs" || p.starts_with("mkfs.") => {
            vec![(Severity::Alert, "formats a filesystem")]
        }
        "chmod" => {
            if has("-R") && has("777") {
                vec![(Severity::Warn, "makes a whole tree world-writable")]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn analyze_git(args: &[&str]) -> Vec<(Severity, &'static str)> {
    const SUBCOMMANDS: &[&str] = &["push", "reset", "clean", "checkout", "restore", "branch"];
    let Some(at) = args.iter().position(|a| SUBCOMMANDS.contains(a)) else {
        return Vec::new();
    };
    let rest = &args[at + 1..];
    let has = |flag: &str| rest.contains(&flag);
    match args[at] {
        "push" => {
            let forced = has("--force")
                || has("-f")
                || rest.iter().any(|a| a.starts_with("--force="))
                || rest.iter().any(|a| a.len() > 1 && a.starts_with('+'));
            if forced {
                vec![(Severity::Alert, "force-pushes, which can overwrite history")]
            } else if has("--force-with-lease") {
                vec![(Severity::Warn, "force-pushes with a lease")]
            } else {
                Vec::new()
            }
        }
        "reset" if has("--hard") => {
            vec![(Severity::Alert, "discards all uncommitted changes")]
        }
        "clean" => {
            let forced = rest
                .iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('f'));
            if forced {
                vec![(Severity::Alert, "deletes untracked files")]
            } else {
                Vec::new()
            }
        }
        "checkout" | "restore" if has(".") && !has("--staged") => {
            vec![(Severity::Warn, "discards changes in the working tree")]
        }
        "branch" if has("-D") => vec![(Severity::Warn, "force-deletes a branch")],
        _ => Vec::new(),
    }
}

fn analyze_rm(args: &[&str]) -> Vec<(Severity, &'static str)> {
    let flags: Vec<&&str> = args.iter().filter(|a| a.starts_with('-')).collect();
    let flag_letters = |letters: &[char]| {
        flags
            .iter()
            .any(|f| !f.starts_with("--") && f.chars().any(|c| letters.contains(&c)))
    };
    let recursive = flag_letters(&['r', 'R']) || flags.iter().any(|f| **f == "--recursive");
    let force = flag_letters(&['f']) || flags.iter().any(|f| **f == "--force");
    if !(recursive && force) {
        return Vec::new();
    }
    let catastrophic = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .any(|a| CATASTROPHIC_TARGETS.contains(&a.to_ascii_lowercase().as_str()));
    if catastrophic {
        vec![(
            Severity::Alert,
            "deletes everything in a top-level location",
        )]
    } else {
        vec![(Severity::Warn, "recursively force-deletes files")]
    }
}

fn tests_failing(events: &[Event]) -> Vec<Finding> {
    let last = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::TestRun { command, passed } => Some((command, *passed)),
        _ => None,
    });
    match last {
        Some((command, Some(false))) => vec![Finding::new(
            "tests-failing",
            Severity::Warn,
            format!("the latest test run failed: {}", shorten(command)),
        )],
        _ => Vec::new(),
    }
}

/// Failed runs of the same test command in a row that make an agent look stuck.
const STUCK_AFTER_FAILURES: usize = 3;

fn repeated_failure(events: &[Event]) -> Vec<Finding> {
    let mut runs = events.iter().rev().filter_map(|e| match &e.kind {
        EventKind::TestRun { command, passed } => Some((command, *passed)),
        _ => None,
    });
    let Some((command, Some(false))) = runs.next() else {
        return Vec::new();
    };
    let streak = 1 + runs
        .take_while(|(c, passed)| *c == command && *passed == Some(false))
        .count();
    if streak < STUCK_AFTER_FAILURES {
        return Vec::new();
    }
    vec![Finding::new(
        "repeated-failure",
        Severity::Warn,
        format!(
            "the same test run has failed {streak} times in a row, so the agent may be stuck: {}",
            shorten(command)
        ),
    )]
}

fn unverified_changes(events: &[Event]) -> Vec<Finding> {
    let Some(last_edit) = events
        .iter()
        .rposition(|e| matches!(e.kind, EventKind::FileEdit { .. }))
    else {
        return Vec::new();
    };
    let verified = events[last_edit..]
        .iter()
        .any(|e| matches!(e.kind, EventKind::TestRun { .. }));
    if verified {
        Vec::new()
    } else {
        vec![Finding::new(
            "unverified-changes",
            Severity::Info,
            "files changed since the last test run (or no tests were run)",
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::FileChange;
    use crate::session::SessionId;

    fn ev(kind: EventKind) -> Event {
        Event::now(SessionId(1), kind)
    }

    fn command(c: &str) -> Event {
        ev(EventKind::Command { command: c.into() })
    }

    fn edit(p: &str) -> Event {
        ev(EventKind::FileEdit { path: p.into() })
    }

    fn test_run(passed: Option<bool>) -> Event {
        ev(EventKind::TestRun {
            command: "cargo test".into(),
            passed,
        })
    }

    fn file(path: &str, kind: ChangeKind, added: &[&str], removed: &[&str]) -> FileChange {
        FileChange {
            path: path.into(),
            kind,
            added: added.iter().map(|s| s.to_string()).collect(),
            removed: removed.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn diff(files: Vec<FileChange>) -> Diff {
        Diff { files }
    }

    fn rules(report: &TrustReport) -> Vec<&str> {
        report.findings.iter().map(|f| f.rule.as_str()).collect()
    }

    fn worst_for_command(c: &str) -> Option<Severity> {
        analyze_command(c).into_iter().map(|(s, _)| s).max()
    }

    // ---- badge and ordering

    #[test]
    fn no_activity_is_green_and_clean() {
        let report = evaluate(&[], None);
        assert_eq!(report, TrustReport::clean());
    }

    #[test]
    fn badge_follows_the_worst_severity() {
        let f = |s| Finding::new("r", s, "x");
        assert_eq!(Badge::from_findings(&[]), Badge::Green);
        assert_eq!(Badge::from_findings(&[f(Severity::Info)]), Badge::Green);
        assert_eq!(
            Badge::from_findings(&[f(Severity::Info), f(Severity::Warn)]),
            Badge::Yellow
        );
        assert_eq!(
            Badge::from_findings(&[f(Severity::Warn), f(Severity::Alert)]),
            Badge::Red
        );
    }

    #[test]
    fn findings_are_sorted_worst_first() {
        let events = [
            edit("a.rs"),
            command("git reset --hard"),
            test_run(Some(false)),
        ];
        let report = evaluate(&events, None);
        let severities: Vec<_> = report.findings.iter().map(|f| f.severity).collect();
        let mut sorted = severities.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(severities, sorted);
        assert_eq!(report.badge, Badge::Red);
    }

    #[test]
    fn names_round_trip() {
        for s in [Severity::Info, Severity::Warn, Severity::Alert] {
            assert_eq!(s.as_str().parse::<Severity>(), Ok(s));
        }
        for b in [Badge::Green, Badge::Yellow, Badge::Red] {
            assert_eq!(b.as_str().parse::<Badge>(), Ok(b));
        }
        assert!("purple".parse::<Badge>().is_err());
    }

    // ---- test path detection

    #[test]
    fn recognises_test_paths() {
        for p in [
            "tests/foo.rs",
            "src/tests/mod.rs",
            "app/__tests__/a.js",
            "spec/models/user_spec.rb",
            "test_utils.py",
            "pkg/foo_test.go",
            "src/a.test.ts",
            "src/a.spec.tsx",
            "src/FooTest.java",
            "src\\tests\\win.rs",
            "e2e/login.ts",
        ] {
            assert!(is_test_path(p), "{p}");
        }
    }

    #[test]
    fn does_not_mistake_ordinary_files_for_tests() {
        for p in [
            "src/lib.rs",
            "src/latest.rs",
            "src/contest.py",
            "README.md",
            "docs/testing.md",
            "src/attestation.rs",
            "",
        ] {
            assert!(!is_test_path(p), "{p}");
        }
    }

    // ---- diff rules

    #[test]
    fn flags_deleted_test_files_as_alerts() {
        let d = diff(vec![
            file("tests/a_test.py", ChangeKind::Deleted, &[], &["assert x"]),
            file("src/a.py", ChangeKind::Deleted, &[], &[]),
        ]);
        let report = evaluate(&[], Some(&d));
        assert_eq!(rules(&report), ["tests-deleted"]);
        assert_eq!(report.badge, Badge::Red);
        assert!(report.findings[0].reason.contains("tests/a_test.py"));
    }

    #[test]
    fn flags_skip_markers_added_to_tests() {
        let d = diff(vec![
            file(
                "tests/a.py",
                ChangeKind::Modified,
                &["@pytest.mark.skip"],
                &[],
            ),
            file(
                "web/a.test.ts",
                ChangeKind::Modified,
                &["it.skip('x', f)"],
                &[],
            ),
            file("src/lib.rs", ChangeKind::Modified, &["#[ignore]"], &[]),
        ]);
        let report = evaluate(&[], Some(&d));
        assert_eq!(rules(&report), ["tests-skipped"; 3]);
        assert_eq!(report.badge, Badge::Yellow);
    }

    #[test]
    fn skip_words_outside_tests_are_not_flagged() {
        let d = diff(vec![
            file("src/app.ts", ChangeKind::Modified, &["list.skip(3)"], &[]),
            file(
                "src/lib.rs",
                ChangeKind::Modified,
                &["// #[ignore] is bad"],
                &[],
            ),
        ]);
        // The Rust comment mentions #[ignore] so the Rust rule still sees it;
        // the TypeScript source file is out of scope.
        let report = evaluate(&[], Some(&d));
        assert!(report.findings.iter().all(|f| !f.reason.contains("app.ts")));
    }

    #[test]
    fn removed_skip_markers_are_not_flagged() {
        let d = diff(vec![file(
            "tests/a.py",
            ChangeKind::Modified,
            &[],
            &["@pytest.mark.skip"],
        )]);
        assert!(evaluate(&[], Some(&d)).findings.is_empty());
    }

    #[test]
    fn flags_net_loss_of_assertions_in_tests() {
        let d = diff(vec![file(
            "tests/a.rs",
            ChangeKind::Modified,
            &["assert_eq!(a, b);"],
            &["assert_eq!(a, 1);", "assert!(ok);", "assert!(done);"],
        )]);
        let report = evaluate(&[], Some(&d));
        assert_eq!(rules(&report), ["assertions-weakened"]);
        assert!(report.findings[0].reason.contains("2 assertion"));
    }

    #[test]
    fn balanced_assertion_changes_are_fine() {
        let d = diff(vec![file(
            "tests/a.rs",
            ChangeKind::Modified,
            &["assert_eq!(a, 2);"],
            &["assert_eq!(a, 1);"],
        )]);
        assert!(evaluate(&[], Some(&d)).findings.is_empty());
    }

    #[test]
    fn assertions_outside_tests_do_not_count() {
        let d = diff(vec![file(
            "src/a.rs",
            ChangeKind::Modified,
            &[],
            &["assert!(x);"],
        )]);
        assert!(evaluate(&[], Some(&d)).findings.is_empty());
    }

    #[test]
    fn flags_always_true_assertions() {
        let d = diff(vec![file(
            "tests/a.rs",
            ChangeKind::Modified,
            &["assert!(true);", "assert!(x);"],
            &["assert_eq!(f(), 3);"],
        )]);
        let report = evaluate(&[], Some(&d));
        assert!(rules(&report).contains(&"trivial-assertion"));
    }

    // ---- sensitive files

    #[test]
    fn flags_secrets_from_events_and_diffs() {
        let events = [
            edit("/home/me/app/.env"),
            edit("C:\\proj\\certs\\server.pem"),
        ];
        let report = evaluate(&events, None);
        assert_eq!(
            rules(&report)
                .iter()
                .filter(|r| **r == "sensitive-file")
                .count(),
            2
        );
        assert_eq!(report.badge, Badge::Red);

        let d = diff(vec![file("deploy/id_rsa", ChangeKind::Added, &["x"], &[])]);
        assert_eq!(evaluate(&[], Some(&d)).badge, Badge::Red);
    }

    #[test]
    fn env_templates_and_public_keys_are_fine() {
        for p in [
            ".env.example",
            ".env.sample",
            "id_rsa.pub",
            "src/envelope.rs",
            "keys.md",
        ] {
            assert_eq!(sensitivity(p), None, "{p}");
        }
    }

    #[test]
    fn ci_config_is_a_warning() {
        let report = evaluate(&[edit(".github/workflows/ci.yml")], None);
        assert_eq!(report.badge, Badge::Yellow);
        assert!(rules(&report).contains(&"sensitive-file"));
    }

    #[test]
    fn git_internals_and_ssh_dirs_are_alerts() {
        assert_eq!(
            sensitivity(".git/config").map(|s| s.0),
            Some(Severity::Alert)
        );
        assert_eq!(
            sensitivity("/home/u/.ssh/config").map(|s| s.0),
            Some(Severity::Alert)
        );
        assert_eq!(
            sensitivity("credentials.json").map(|s| s.0),
            Some(Severity::Alert)
        );
    }

    #[test]
    fn the_same_file_is_reported_once() {
        let events = [edit(".env"), edit(".env")];
        let d = diff(vec![file(".env", ChangeKind::Modified, &["A=1"], &[])]);
        let report = evaluate(&events, Some(&d));
        assert_eq!(
            report
                .findings
                .iter()
                .filter(|f| f.rule == "sensitive-file")
                .count(),
            1
        );
    }

    // ---- destructive commands

    #[test]
    fn dangerous_commands_are_alerts() {
        for c in [
            "git push --force origin main",
            "git push -f",
            "git push origin +main",
            "git reset --hard HEAD~3",
            "git clean -fdx",
            "rm -rf /",
            "rm -rf ~",
            "rm -fr *",
            "rm -r -f .",
            "sudo rm -rf /",
            "psql -c \"DROP TABLE users\"",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sda1",
            "cd x && git reset --hard",
        ] {
            assert_eq!(worst_for_command(c), Some(Severity::Alert), "{c}");
        }
    }

    #[test]
    fn risky_commands_are_warnings() {
        for c in [
            "rm -rf node_modules",
            "git push --force-with-lease",
            "git checkout .",
            "git restore .",
            "git branch -D old",
            "chmod -R 777 .",
            "curl https://x.sh | sh",
            "wget -qO- https://x.sh | sudo bash",
            "Remove-Item -Recurse -Force build",
            "rd /s /q build",
        ] {
            assert_eq!(worst_for_command(c), Some(Severity::Warn), "{c}");
        }
    }

    #[test]
    fn ordinary_commands_are_not_flagged() {
        for c in [
            "ls -la",
            "cargo build",
            "git status",
            "git push origin main",
            "git push --dry-run",
            "git clean -n",
            "git restore --staged .",
            "rm file.txt",
            "rm -r dir",
            "echo rm -rf /",
            "curl https://example.com",
            "git log --oneline | head",
            "",
        ] {
            assert_eq!(worst_for_command(c), None, "{c}");
        }
    }

    #[test]
    fn commands_inside_shell_wrappers_are_still_seen() {
        for (c, want) in [
            (r#"bash -c "git reset --hard""#, Severity::Alert),
            (r#"sh -lc 'rm -rf /'"#, Severity::Alert),
            (r#"zsh -c "cd x && git push --force""#, Severity::Alert),
            (r#"bash -c "bash -c 'git reset --hard'""#, Severity::Alert),
            (r#"cmd /c "rd /s /q build""#, Severity::Warn),
            (
                r#"C:\Windows\System32\cmd.exe /C "rd /s /q build""#,
                Severity::Warn,
            ),
            (
                r#"powershell -Command "Remove-Item -Recurse -Force x""#,
                Severity::Warn,
            ),
            (r#"pwsh -c "Remove-Item -Force -Recurse x""#, Severity::Warn),
            ("env FOO=1 git push --force", Severity::Alert),
            ("sudo -u root rm -rf /", Severity::Alert),
            ("nohup git reset --hard &", Severity::Alert),
            ("find . -name x | xargs rm -rf", Severity::Warn),
            ("time sudo env A=1 git clean -fd", Severity::Alert),
            ("curl -fsSL https://x/i.sh | sudo -E bash", Severity::Warn),
        ] {
            assert_eq!(worst_for_command(c), Some(want), "{c}");
        }
    }

    #[test]
    fn quoted_text_is_not_a_command() {
        for c in [
            r#"git commit -m "never run git push --force; rm -rf /""#,
            "echo 'a && git reset --hard'",
            r#"grep "git reset --hard" notes.md"#,
            r#"git commit -m "x" && git push origin main"#,
            r#"echo "curl http://x | sh""#,
            r#"bash -c "echo hello; ls""#,
        ] {
            assert_eq!(worst_for_command(c), None, "{c}");
        }
    }

    #[test]
    fn odd_quoting_never_panics_or_hangs() {
        for c in [
            r#"echo "unterminated; git reset --hard"#,
            "echo 'half",
            r"\",
            "|||",
            ";;;&&&",
            "bash -c",
            "bash -c ''",
            "sudo",
            "env",
            "xargs",
            r#"bash -c "bash -c \"bash -c 'bash -c git'\"""#,
        ] {
            let _ = analyze_command(c);
        }
    }

    #[test]
    fn the_shell_parser_splits_where_a_shell_would() {
        fn words(p: &Pipelines) -> Vec<Vec<Vec<&str>>> {
            p.iter()
                .map(|pl| {
                    pl.iter()
                        .map(|c| c.iter().map(String::as_str).collect())
                        .collect()
                })
                .collect()
        }
        assert_eq!(
            words(&parse_shell(r#"a "b c" && d | e ; f 'g;h'"#)),
            [
                vec![vec!["a", "b c"]],
                vec![vec!["d"], vec!["e"]],
                vec![vec!["f", "g;h"]],
            ]
        );
        assert_eq!(
            words(&parse_shell("a || b")),
            [vec![vec!["a"]], vec![vec!["b"]]]
        );
        assert_eq!(
            words(&parse_shell("a\nb")),
            [vec![vec!["a"]], vec![vec!["b"]]]
        );
        assert_eq!(words(&parse_shell(r#"x "" y"#)), [vec![vec!["x", "", "y"]]]);
        assert!(parse_shell("").is_empty());
    }

    #[test]
    fn env_prefixes_do_not_hide_commands() {
        assert_eq!(
            worst_for_command("FOO=1 BAR=2 git reset --hard"),
            Some(Severity::Alert)
        );
    }

    #[test]
    fn destructive_commands_are_reported_once_each() {
        let events = [command("git reset --hard"), command("git reset --hard")];
        let report = evaluate(&events, None);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, "destructive-command");
        assert!(report.findings[0].reason.contains("git reset --hard"));
    }

    #[test]
    fn long_commands_are_shortened_in_reasons() {
        let long = format!("git push --force {}", "x".repeat(300));
        let report = evaluate(&[command(&long)], None);
        assert!(report.findings[0].reason.chars().count() < 200);
    }

    // ---- test outcomes

    #[test]
    fn a_failing_latest_test_run_is_a_warning() {
        let report = evaluate(&[edit("a.rs"), test_run(Some(false))], None);
        assert_eq!(rules(&report), ["tests-failing"]);
        assert_eq!(report.badge, Badge::Yellow);
    }

    #[test]
    fn a_later_passing_run_clears_the_failure() {
        let events = [test_run(Some(false)), edit("a.rs"), test_run(Some(true))];
        assert!(evaluate(&events, None).findings.is_empty());
    }

    fn test_run_of(command: &str, passed: Option<bool>) -> Event {
        ev(EventKind::TestRun {
            command: command.into(),
            passed,
        })
    }

    #[test]
    fn three_identical_failures_in_a_row_look_stuck() {
        let events = [
            test_run(Some(false)),
            edit("a.rs"),
            test_run(Some(false)),
            test_run(Some(false)),
        ];
        let report = evaluate(&events, None);
        let found: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule == "repeated-failure" || f.rule == "tests-failing")
            .map(|f| f.rule.as_str())
            .collect();
        // The streak replaces the plain failure rather than adding to it.
        assert_eq!(found, ["repeated-failure"]);
        assert!(report.findings[0].reason.contains("3 times"));
        assert_eq!(report.badge, Badge::Yellow);
    }

    #[test]
    fn two_failures_a_pass_or_a_different_command_are_not_stuck() {
        let two = [test_run(Some(false)), test_run(Some(false))];
        assert_eq!(rules(&evaluate(&two, None)), ["tests-failing"]);

        let healed = [
            test_run(Some(false)),
            test_run(Some(false)),
            test_run(Some(true)),
            test_run(Some(false)),
        ];
        assert_eq!(rules(&evaluate(&healed, None)), ["tests-failing"]);

        let varied = [
            test_run_of("cargo test a", Some(false)),
            test_run_of("cargo test b", Some(false)),
            test_run_of("cargo test a", Some(false)),
        ];
        assert_eq!(rules(&evaluate(&varied, None)), ["tests-failing"]);

        let unknown = [
            test_run(Some(false)),
            test_run(None),
            test_run(Some(false)),
            test_run(Some(false)),
        ];
        assert_eq!(rules(&evaluate(&unknown, None)), ["tests-failing"]);
    }

    #[test]
    fn unknown_outcomes_are_not_failures() {
        assert!(evaluate(&[test_run(None)], None).findings.is_empty());
    }

    #[test]
    fn edits_without_a_later_test_run_are_info_only() {
        let report = evaluate(&[edit("a.rs")], None);
        assert_eq!(rules(&report), ["unverified-changes"]);
        assert_eq!(report.badge, Badge::Green);

        let report = evaluate(&[test_run(Some(true)), edit("a.rs")], None);
        assert_eq!(rules(&report), ["unverified-changes"]);

        let report = evaluate(&[edit("a.rs"), test_run(Some(true))], None);
        assert!(report.findings.is_empty());
    }

    // ---- business-sensitive paths

    #[test]
    fn auth_payments_and_migrations_are_warnings() {
        for p in [
            "src/auth/session.rs",
            "app/controllers/LoginController.java",
            "pkg/oauth2/client.go",
            "services/OAuthService.ts",
            "src/billing/invoice.py",
            "lib/payments.rb",
            "db/migrate/20260101_add_users.rb",
            "migrations/0001_init.sql",
        ] {
            let report = evaluate(&[edit(p)], None);
            assert!(rules(&report).contains(&"sensitive-file"), "{p}");
            assert_eq!(report.badge, Badge::Yellow, "{p}");
        }
    }

    #[test]
    fn lookalike_words_tests_and_docs_are_not_business_sensitive() {
        for p in [
            "src/author.rs",
            "src/authors/list.ts",
            "src/paymentless.rs",
            "docs/auth.md",
            "tests/auth_test.py",
            "src/emigrate.rs",
        ] {
            assert_eq!(business_sensitivity(p), None, "{p}");
        }
    }

    #[test]
    fn the_project_folder_name_does_not_make_every_file_sensitive() {
        let observed = Observations {
            root: Some("/home/me/payments-api".into()),
            ..Observations::default()
        };
        let events = [edit("/home/me/payments-api/src/main.rs")];
        let report = evaluate_with(&events, None, &TrustConfig::default(), &observed);
        assert!(report.findings.iter().all(|f| f.rule != "sensitive-file"));

        let events = [edit("/home/me/payments-api/src/billing/charge.rs")];
        let report = evaluate_with(&events, None, &TrustConfig::default(), &observed);
        assert!(rules(&report).contains(&"sensitive-file"));
    }

    #[test]
    fn absolute_and_relative_paths_to_one_file_are_reported_once() {
        let observed = Observations {
            root: Some("C:\\work\\app".into()),
            ..Observations::default()
        };
        let events = [edit("C:\\work\\app\\.env")];
        let d = diff(vec![file(".env", ChangeKind::Modified, &["A=1"], &[])]);
        let report = evaluate_with(&events, Some(&d), &TrustConfig::default(), &observed);
        let sensitive: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule == "sensitive-file")
            .collect();
        assert_eq!(sensitive.len(), 1);
        assert!(sensitive[0].reason.ends_with(": .env"));
    }

    // ---- user configuration

    #[test]
    fn relative_to_strips_root_and_ignores_paths_outside_it() {
        let root = Some("/home/me/app");
        assert_eq!(relative_to("/home/me/app/src/a.rs", root), "src/a.rs");
        assert_eq!(relative_to("/HOME/me/app/src/a.rs", root), "src/a.rs");
        assert_eq!(relative_to("/elsewhere/a.rs", root), "/elsewhere/a.rs");
    }

    #[test]
    fn relative_to_does_not_panic_on_multibyte_paths_outside_root() {
        // Root is 12 bytes; byte 12 of this path falls inside the second `é`.
        let root = Some("/home/me/app");
        assert_eq!(
            relative_to("docs/résumé.md", root),
            "docs/résumé.md".to_string()
        );
        // A multi-byte path that is under the root still resolves.
        assert_eq!(
            relative_to("/home/me/app/docs/résumé.md", root),
            "docs/résumé.md"
        );
    }

    #[test]
    fn globs_match_segments_and_depth() {
        assert!(glob_match("src/ledger/**", "src/ledger/a/b.rs"));
        assert!(glob_match("src/ledger/**", "/abs/repo/src/ledger/x.rs"));
        assert!(!glob_match("/src/ledger/**", "/abs/repo/src/ledger/x.rs"));
        assert!(glob_match("*.sql", "db/schema.sql"));
        assert!(glob_match("**/kyc/*.py", "svc/kyc/check.py"));
        assert!(glob_match("Config?.toml", "config1.TOML"));
        assert!(glob_match("src\\vault\\**", "src/vault/key.rs"));
        assert!(!glob_match("*.sql", "db/schema.sqlite"));
        assert!(!glob_match("src/*.rs", "src/a/b.rs"));
        assert!(!glob_match("", "anything"));
    }

    #[test]
    fn user_patterns_add_findings_with_their_severity() {
        let config = TrustConfig {
            sensitive_paths: vec![PathRule {
                pattern: "src/ledger/**".into(),
                severity: Severity::Alert,
            }],
            ignore_paths: Vec::new(),
        };
        let observed = Observations::default();
        let report = evaluate_with(&[edit("src/ledger/post.rs")], None, &config, &observed);
        assert_eq!(report.badge, Badge::Red);
        assert!(report.findings[0].reason.contains("src/ledger/**"));
    }

    #[test]
    fn ignored_paths_are_never_sensitive() {
        let config = TrustConfig {
            sensitive_paths: Vec::new(),
            ignore_paths: vec!["src/auth/**".into(), ".env.local".into()],
        };
        let observed = Observations::default();
        let events = [edit("src/auth/session.rs"), edit(".env.local")];
        let report = evaluate_with(&events, None, &config, &observed);
        assert!(report.findings.iter().all(|f| f.rule != "sensitive-file"));
    }

    // ---- coverage thresholds

    #[test]
    fn lowering_a_coverage_threshold_is_an_alert() {
        let cases = [
            ("pyproject.toml", "fail_under = 90", "fail_under = 40"),
            (
                "setup.cfg",
                "addopts = --cov-fail-under=85",
                "addopts = --cov-fail-under=50",
            ),
            ("jest.config.js", "    branches: 80,", "    branches: 20,"),
            ("codecov.yml", "        target: 90%", "        target: 75%"),
            (
                ".github/workflows/ci.yml",
                "  run: cargo llvm-cov --fail-under-lines 80",
                "  run: cargo llvm-cov --fail-under-lines 70",
            ),
        ];
        for (path, old, new) in cases {
            let d = diff(vec![file(path, ChangeKind::Modified, &[new], &[old])]);
            let report = evaluate(&[], Some(&d));
            assert!(rules(&report).contains(&"coverage-lowered"), "{path}");
            assert_eq!(report.badge, Badge::Red, "{path}");
        }
    }

    #[test]
    fn raising_or_unrelated_numbers_are_not_coverage_findings() {
        let cases = [
            ("pyproject.toml", "fail_under = 40", "fail_under = 90"),
            ("src/retry.rs", "let threshold = 5;", "let threshold = 3;"),
            (
                "jest.config.js",
                "  testTimeout: 5000,",
                "  testTimeout: 1000,",
            ),
            ("src/app.ts", "const lines = 80;", "const lines = 20;"),
            ("README.md", "coverage: 90%", "coverage: 40%"),
        ];
        for (path, old, new) in cases {
            let d = diff(vec![file(path, ChangeKind::Modified, &[new], &[old])]);
            assert!(
                !rules(&evaluate(&[], Some(&d))).contains(&"coverage-lowered"),
                "{path}"
            );
        }
    }

    #[test]
    fn numbers_are_templated() {
        assert_eq!(
            numbers_and_template("fail_under = 92.5,"),
            (vec![92.5], "fail_under = #".to_string())
        );
        assert_eq!(
            numbers_and_template("v1 and 2."),
            (vec![1.0, 2.0], "v# and #.".to_string())
        );
    }

    // ---- suppressions and lockfiles

    #[test]
    fn added_suppressions_are_warnings() {
        let d = diff(vec![
            file(
                "src/a.ts",
                ChangeKind::Modified,
                &["// eslint-disable-next-line", "// @ts-ignore"],
                &[],
            ),
            file(
                "app/b.py",
                ChangeKind::Modified,
                &["x = f()  # type: ignore"],
                &[],
            ),
            file(
                "src/c.rs",
                ChangeKind::Modified,
                &["#[allow(dead_code)]"],
                &[],
            ),
        ]);
        let report = evaluate(&[], Some(&d));
        let found: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule == "checks-suppressed")
            .collect();
        assert_eq!(found.len(), 3);
        assert!(found[0].reason.contains("2 lint"));
        assert_eq!(report.badge, Badge::Yellow);
    }

    #[test]
    fn removed_suppressions_and_docs_are_fine() {
        let d = diff(vec![
            file(
                "src/a.ts",
                ChangeKind::Modified,
                &[],
                &["// eslint-disable"],
            ),
            file(
                "TRUST.md",
                ChangeKind::Modified,
                &["`# noqa` is flagged"],
                &[],
            ),
        ]);
        assert!(evaluate(&[], Some(&d)).findings.is_empty());
    }

    #[test]
    fn lockfile_changes_are_info_and_listed_once() {
        let d = diff(vec![
            file("Cargo.lock", ChangeKind::Modified, &["x"], &[]),
            file("web/package-lock.json", ChangeKind::Modified, &["x"], &[]),
            file("src/lib.rs", ChangeKind::Modified, &["x"], &[]),
        ]);
        let report = evaluate(&[], Some(&d));
        assert_eq!(rules(&report), ["lockfile-changed"]);
        assert_eq!(report.badge, Badge::Green);
        assert!(report.findings[0].reason.contains("web/package-lock.json"));
    }

    #[test]
    fn manifest_changes_are_info_and_other_files_are_not_manifests() {
        let d = diff(vec![
            file("Cargo.toml", ChangeKind::Modified, &["x"], &[]),
            file("web/package.json", ChangeKind::Modified, &["x"], &[]),
            file("requirements-dev.txt", ChangeKind::Added, &["x"], &[]),
            file("src/lib.rs", ChangeKind::Modified, &["x"], &[]),
            file("notes.txt", ChangeKind::Modified, &["x"], &[]),
        ]);
        let report = evaluate(&[], Some(&d));
        assert_eq!(rules(&report), ["manifest-changed"]);
        assert_eq!(report.badge, Badge::Green);
        let reason = &report.findings[0].reason;
        assert!(reason.contains("Cargo.toml") && reason.contains("requirements-dev.txt"));
        assert!(!reason.contains("notes.txt") && !reason.contains("lib.rs"));

        let none = diff(vec![file("src/main.rs", ChangeKind::Modified, &["x"], &[])]);
        assert!(evaluate(&[], Some(&none)).findings.is_empty());
    }

    #[test]
    fn edits_outside_the_session_directory_are_flagged() {
        let obs = Observations {
            root: Some("/work/app".into()),
            ..Observations::default()
        };
        let cfg = TrustConfig::default();
        let run = |paths: &[&str]| {
            let events: Vec<Event> = paths.iter().map(|p| edit(p)).collect();
            evaluate_with(&events, None, &cfg, &obs)
        };

        let report = run(&["/work/app/src/a.rs", "/home/me/.bashrc", "/home/me/.bashrc"]);
        let outside: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule == "outside-project")
            .collect();
        assert_eq!(outside.len(), 1);
        assert_eq!(outside[0].severity, Severity::Warn);
        assert!(outside[0].reason.contains("/home/me/.bashrc"));
        assert!(!outside[0].reason.contains("src/a.rs"));

        // Inside, relative, scratch space and agent notes are all fine.
        for ok in [
            "/work/app/src/a.rs",
            "/WORK/app/a.rs",
            "src/a.rs",
            "/tmp/scratch.txt",
            "/private/var/folders/xy/T/out.txt",
            "C:/Users/me/AppData/Local/Temp/x.txt",
            "/home/me/.claude/projects/p/memory/m.md",
        ] {
            assert!(
                !run(&[ok])
                    .findings
                    .iter()
                    .any(|f| f.rule == "outside-project"),
                "{ok} should not be flagged"
            );
        }
        // A sibling that merely shares a prefix is outside.
        assert!(run(&["/work/app-other/a.rs"])
            .findings
            .iter()
            .any(|f| f.rule == "outside-project"));
        // Without a known root there is nothing to compare against.
        assert!(!evaluate(&[edit("/home/me/.bashrc")], None)
            .findings
            .iter()
            .any(|f| f.rule == "outside-project"));
    }

    #[test]
    fn ignored_paths_are_not_reported_as_outside() {
        let obs = Observations {
            root: Some("/work/app".into()),
            ..Observations::default()
        };
        let cfg = TrustConfig {
            ignore_paths: vec!["/home/me/notes/**".into()],
            ..TrustConfig::default()
        };
        let report = evaluate_with(&[edit("/home/me/notes/a.md")], None, &cfg, &obs);
        assert!(!report.findings.iter().any(|f| f.rule == "outside-project"));
    }

    #[test]
    fn shared_edits_pairs_files_with_the_other_session() {
        let set = |paths: &[&str]| paths.iter().map(|p| p.to_string()).collect::<BTreeSet<_>>();
        let mine = set(&["/a/x.rs", "/a/y.rs"]);
        let theirs = set(&["/a/y.rs", "/a/z.rs"]);
        let shared = shared_edits(&mine, &[("other", &theirs)]);
        assert_eq!(
            shared,
            [SharedEdit {
                path: "/a/y.rs".into(),
                session: "other".into()
            }]
        );
        assert!(shared_edits(&mine, &[]).is_empty());
        assert_eq!(edited_path("C:\\p\\a.rs").as_deref(), Some("C:/p/a.rs"));
        assert_eq!(edited_path("src/a.rs"), None);
    }

    #[test]
    fn shared_edits_become_a_session_overlap_warning() {
        let obs = Observations {
            shared_edits: vec![SharedEdit {
                path: "/a/y.rs".into(),
                session: "other".into(),
            }],
            ..Observations::default()
        };
        let report = evaluate_with(&[], None, &TrustConfig::default(), &obs);
        assert_eq!(rules(&report), ["session-overlap"]);
        assert_eq!(report.badge, Badge::Yellow);
        assert!(report.findings[0].reason.contains("/a/y.rs"));
        assert!(evaluate(&[], None).findings.is_empty());
    }

    #[test]
    fn credentials_in_added_lines_are_an_alert_that_never_shows_the_secret() {
        let key = "ghp_abcdEFGH1234abcdEFGH1234abcdEFGH1234";
        let d = diff(vec![
            file(
                "src/config.py",
                ChangeKind::Modified,
                &[&format!("TOKEN = \"{key}\"")],
                &[],
            ),
            file(
                "deploy/id.pem",
                ChangeKind::Added,
                &["-----BEGIN RSA PRIVATE KEY-----"],
                &[],
            ),
            file(
                "db.rs",
                ChangeKind::Modified,
                &["let url = \"postgres://admin:hunter2@db.internal/app\";"],
                &[],
            ),
            file(
                "aws.txt",
                ChangeKind::Added,
                &["id=AKIAABCDEFGHIJKLMNOP"],
                &[],
            ),
        ]);
        let report = evaluate(&[], Some(&d));
        let found: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule == "secret-added")
            .collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity, Severity::Alert);
        assert_eq!(report.badge, Badge::Red);
        let reason = &found[0].reason;
        assert!(reason.contains("src/config.py") && reason.contains("and 1 more"));
        assert!(!reason.contains(key) && !reason.contains("hunter2"));
    }

    #[test]
    fn ordinary_code_placeholders_and_removed_lines_are_not_secrets() {
        let d = diff(vec![
            file(
                "a.rs",
                ChangeKind::Modified,
                &[
                    "let token = next_token();",
                    "max_tokens=100",
                    "let id = \"sk-1\";",
                    "echo $npm_config_user_agent",
                    "pipeline = \"sk-learn-pipeline-transformer\"",
                    "TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                    "api_key = \"sk-ant-YOUR-KEY-GOES-HERE-0000\"",
                    "see https://example.com/a and https://user@host/x",
                    "url = \"postgres://user:password@example.com/db\"",
                ],
                &[],
            ),
            file(
                "old.env",
                ChangeKind::Modified,
                &[],
                &["TOKEN=ghp_abcdEFGH1234abcdEFGH1234abcdEFGH1234"],
            ),
        ]);
        assert!(evaluate(&[], Some(&d))
            .findings
            .iter()
            .all(|f| f.rule != "secret-added"));
    }

    fn many_files(n: usize) -> Vec<FileChange> {
        (0..n)
            .map(|i| file(&format!("src/f{i}.rs"), ChangeKind::Modified, &["x"], &[]))
            .collect()
    }

    #[test]
    fn big_changes_are_info_and_huge_ones_a_warning() {
        let report = evaluate(&[], Some(&diff(many_files(LARGE_FILES))));
        let big: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule == "large-change")
            .collect();
        assert_eq!(big.len(), 1);
        assert_eq!(big[0].severity, Severity::Info);
        assert!(big[0].reason.contains("25 files"));

        let lines: Vec<String> = (0..LARGE_LINES).map(|i| format!("l{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = diff(vec![file("big.rs", ChangeKind::Added, &refs, &[])]);
        let report = evaluate(&[], Some(&d));
        assert_eq!(rules(&report), ["large-change"]);
        assert_eq!(report.badge, Badge::Green);

        let huge = evaluate(&[], Some(&diff(many_files(LARGE_FILES * HUGE_FACTOR))));
        let f = huge
            .findings
            .iter()
            .find(|f| f.rule == "large-change")
            .unwrap();
        assert_eq!(f.severity, Severity::Warn);
        assert_eq!(huge.badge, Badge::Yellow);
    }

    #[test]
    fn small_changes_and_lockfile_churn_are_not_large() {
        let small = evaluate(&[], Some(&diff(many_files(LARGE_FILES - 1))));
        assert!(small.findings.iter().all(|f| f.rule != "large-change"));

        // A lockfile can change thousands of lines without it being a big change.
        let lines: Vec<String> = (0..5000).map(|i| format!("l{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = diff(vec![file("Cargo.lock", ChangeKind::Modified, &refs, &refs)]);
        let report = evaluate(&[], Some(&d));
        assert_eq!(rules(&report), ["lockfile-changed"]);
    }

    // ---- hook silence

    fn busy_claude() -> Observations {
        Observations {
            hooks_expected: true,
            output_bytes: HOOK_SILENCE_OUTPUT_BYTES,
            running_secs: HOOK_SILENCE_SECS,
            root: None,
            ..Observations::default()
        }
    }

    #[test]
    fn the_silent_hooks_finding_tells_codex_users_what_to_do() {
        let report = evaluate_with(&[], None, &TrustConfig::default(), &busy_claude());
        let finding = report
            .findings
            .iter()
            .find(|f| f.rule == "hooks-silent")
            .unwrap();
        assert!(
            finding.reason.contains("sakshi setup codex"),
            "{}",
            finding.reason
        );
    }

    #[test]
    fn a_busy_agent_with_silent_hooks_is_flagged() {
        let report = evaluate_with(&[], None, &TrustConfig::default(), &busy_claude());
        assert_eq!(rules(&report), ["hooks-silent"]);
        assert_eq!(report.badge, Badge::Yellow);
    }

    #[test]
    fn hooks_are_not_silent_once_anything_is_reported() {
        let events = [ev(EventKind::State(crate::session::SessionState::Working))];
        assert!(!hooks_silent(&events, &busy_claude()));
    }

    #[test]
    fn hook_silence_needs_time_output_and_an_agent_with_hooks() {
        let quiet = Observations {
            output_bytes: 10,
            ..busy_claude()
        };
        let young = Observations {
            running_secs: 5,
            ..busy_claude()
        };
        let shell = Observations {
            hooks_expected: false,
            ..busy_claude()
        };
        for o in [quiet, young, shell] {
            assert!(!hooks_silent(&[], &o), "{o:?}");
        }
        // Sakshi's own bookkeeping is not a hook report.
        let bookkeeping = [ev(EventKind::Trust {
            badge: Badge::Green,
            findings: Vec::new(),
        })];
        assert!(hooks_silent(&bookkeeping, &busy_claude()));
    }

    /// Rule names from the first column of the tables in TRUST.md.
    fn documented_rules() -> BTreeSet<String> {
        include_str!("../../../TRUST.md")
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn every_documented_rule_has_a_concern_and_nothing_else_does() {
        let documented = documented_rules();
        assert_eq!(documented.len(), 18, "{documented:?}");
        for rule in &documented {
            assert!(
                Concern::of_known_rule(rule).is_some(),
                "{rule} is in TRUST.md but has no concern"
            );
        }
        // The engine's own source names no rule that TRUST.md leaves out.
        let source = include_str!("trust.rs");
        let engine = source.split("\n#[cfg(test)]").next().unwrap();
        for rule in engine
            .split("Finding::new(")
            .skip(1)
            .filter_map(|rest| rest.trim_start().strip_prefix('"'))
            .filter_map(|rest| rest.split('"').next())
        {
            assert!(documented.contains(rule), "{rule} is not in TRUST.md");
        }
    }

    #[test]
    fn an_unknown_rule_is_something_to_review_not_an_error() {
        assert_eq!(Concern::of_known_rule("from-the-future"), None);
        assert_eq!(Concern::of_rule("from-the-future"), Concern::Review);
    }

    #[test]
    fn findings_group_by_concern_in_a_fixed_order() {
        let events = [command("rm -rf /"), edit("src/a.rs"), test_run(Some(false))];
        let d = Diff {
            files: vec![
                file("tests/a_test.py", ChangeKind::Deleted, &[], &["assert x"]),
                file("Cargo.lock", ChangeKind::Modified, &["a"], &["b"]),
            ],
        };
        let report = evaluate(&events, Some(&d));
        let groups = report.by_concern();
        let order: Vec<Concern> = groups.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            order,
            [
                Concern::FakedSuccess,
                Concern::RiskyAction,
                Concern::Unverified,
                Concern::Review
            ]
        );
        // Every finding lands in exactly one group.
        let grouped: usize = groups.iter().map(|(_, g)| g.len()).sum();
        assert_eq!(grouped, report.findings.len());
        assert!(TrustReport::clean().by_concern().is_empty());
    }

    #[test]
    fn the_worst_finding_decides_and_notes_never_do() {
        let events = [command("rm -rf /")];
        let d = Diff {
            files: vec![file(
                "tests/a_test.py",
                ChangeKind::Deleted,
                &[],
                &["assert x"],
            )],
        };
        // Two alerts: faking success is named before a risky action.
        let report = evaluate(&events, Some(&d));
        assert_eq!(report.worst().unwrap().rule, "tests-deleted");
        // Only a note: the verdict is green and nothing is "worst".
        let lock = Diff {
            files: vec![file("Cargo.lock", ChangeKind::Modified, &["a"], &["b"])],
        };
        let notes = evaluate(&[], Some(&lock));
        assert_eq!(notes.badge, Badge::Green);
        assert!(notes.worst().is_none());
    }
}
