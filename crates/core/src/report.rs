//! Reports and replays built from a session's recorded events.

use time::OffsetDateTime;

use crate::event::{format_age, Event, EventKind};
use crate::session::{AgentKind, SessionId, SessionState};
use crate::summary::{summarize, Summary};
use crate::trust::{evaluate, Badge, TrustReport};

/// Commands listed in a report before "and N more".
const MAX_COMMANDS_LISTED: usize = 15;

/// Everything worth saying about a finished (or ongoing) session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The session.
    pub session_id: SessionId,
    /// Display name, from the session record.
    pub name: String,
    /// Agent kind, from the session record.
    pub agent: String,
    /// Working directory, from the session record.
    pub cwd: String,
    /// First event time.
    pub started: OffsetDateTime,
    /// Last event time.
    pub ended: OffsetDateTime,
    /// The latest state the session reported.
    pub final_state: Option<SessionState>,
    /// What happened, over the whole session.
    pub summary: Summary,
    /// The last recorded trust verdict, or one computed from the events.
    pub trust: TrustReport,
    /// Number of events recorded.
    pub event_count: usize,
}

impl Report {
    /// Build a report from a session's events, oldest first. `None` if there
    /// are none.
    pub fn from_events(events: &[Event]) -> Option<Report> {
        let first = events.first()?;
        let last = events.last()?;
        let (name, agent, cwd) = events
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::Session { name, agent, cwd } => {
                    Some((name.clone(), agent.clone(), cwd.clone()))
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                (
                    first.session_id.to_string(),
                    "unknown".to_string(),
                    String::new(),
                )
            });
        // The recorded verdict also knows about diffs; fall back to the
        // event-only rules for logs that never recorded one.
        let trust = events
            .iter()
            .rev()
            .find_map(|e| match &e.kind {
                EventKind::Trust { badge, findings } => Some(TrustReport {
                    badge: *badge,
                    findings: findings.clone(),
                }),
                _ => None,
            })
            .unwrap_or_else(|| evaluate(events, None));
        let summary = summarize(events, None);
        Some(Report {
            session_id: first.session_id,
            name,
            agent,
            cwd,
            started: first.ts,
            ended: last.ts,
            final_state: summary.state,
            summary,
            trust,
            event_count: events.len(),
        })
    }

    /// Seconds between the first and last event.
    pub fn duration_seconds(&self) -> i64 {
        (self.ended - self.started).whole_seconds().max(0)
    }

    /// The report as a Markdown document.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# Sakshi report: {}\n\n", self.name));
        out.push_str(&format!("- Session: `{}`\n", self.session_id));
        out.push_str(&format!("- Agent: {}\n", self.agent));
        if !self.cwd.is_empty() {
            out.push_str(&format!("- Directory: `{}`\n", self.cwd));
        }
        out.push_str(&format!("- Started: {}\n", self.started));
        out.push_str(&format!(
            "- Duration: {}\n",
            format_age(self.duration_seconds())
        ));
        if let Some(state) = self.final_state {
            out.push_str(&format!("- Final state: {state}\n"));
        }
        out.push_str(&format!("- Events recorded: {}\n\n", self.event_count));

        out.push_str(&format!(
            "## Trust: {}\n\n",
            self.trust.badge.as_str().to_uppercase()
        ));
        out.push_str(&findings_markdown(&self.trust, |title| {
            format!("### {title}")
        }));

        out.push_str("## What happened\n\n");
        let lines = self.summary.lines();
        if lines.is_empty() {
            out.push_str("No activity was recorded.\n");
        }
        for line in lines {
            out.push_str(&format!("- {line}\n"));
        }

        if !self.summary.files_edited.is_empty() {
            out.push_str("\n## Files edited\n\n");
            for file in &self.summary.files_edited {
                out.push_str(&format!("- `{}`\n", code_safe(file)));
            }
        }
        if !self.summary.commands.is_empty() {
            out.push_str("\n## Commands\n\n");
            for command in self.summary.commands.iter().take(MAX_COMMANDS_LISTED) {
                out.push_str(&format!("- `{}`\n", code_safe(command)));
            }
            let more = self
                .summary
                .commands
                .len()
                .saturating_sub(MAX_COMMANDS_LISTED);
            if more > 0 {
                out.push_str(&format!("- ...and {more} more\n"));
            }
        }
        out
    }
}

/// Files listed in a pull-request description before "and N more".
const MAX_PR_FILES: usize = 20;

impl Report {
    /// A short Markdown block to paste into a pull request description: the
    /// verdict, the findings and the files touched, with paths made relative
    /// to the session directory and no local ids or timestamps.
    pub fn to_pr_markdown(&self) -> String {
        let mut out = String::from("### Agent changes (recorded by Sakshi)\n\n");
        let duration = format_age(self.duration_seconds());
        out.push_str(&format!(
            "{} ran for {duration}. **Trust: {}**\n\n",
            self.agent,
            self.trust.badge.as_str().to_uppercase()
        ));
        out.push_str(&findings_markdown(&self.trust, |title| {
            format!("**{title}**")
        }));
        let s = &self.summary;
        if s.tests_passed + s.tests_failed + s.tests_unknown == 0 {
            out.push_str("**Tests:** the agent ran none.\n");
        } else {
            out.push_str(&format!(
                "**Tests (run by the agent):** {} passed, {} failed, {} unknown.\n",
                s.tests_passed, s.tests_failed, s.tests_unknown
            ));
        }
        if !s.files_edited.is_empty() {
            out.push_str(&format!("\n**Files edited ({}):**\n", s.files_edited.len()));
            for file in s.files_edited.iter().take(MAX_PR_FILES) {
                out.push_str(&format!("- `{}`\n", code_safe(&relative(file, &self.cwd))));
            }
            let more = s.files_edited.len().saturating_sub(MAX_PR_FILES);
            if more > 0 {
                out.push_str(&format!("- ...and {more} more\n"));
            }
        }
        out
    }
}

/// The findings as Markdown lists, one per concern, each under `heading`.
fn findings_markdown(trust: &TrustReport, heading: impl Fn(&str) -> String) -> String {
    if trust.findings.is_empty() {
        return "Nothing looked off.\n\n".to_string();
    }
    let mut out = String::new();
    for (concern, findings) in trust.by_concern() {
        out.push_str(&heading(concern.title()));
        out.push_str("\n\n");
        for finding in findings {
            out.push_str(&format!(
                "- **{}** `{}`: {}\n",
                finding.severity.as_str(),
                finding.rule,
                finding.reason
            ));
        }
        out.push('\n');
    }
    out
}

/// What Sakshi concluded about one session, for the summary printed on exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionVerdict {
    /// The session.
    pub id: SessionId,
    /// Its display name.
    pub name: String,
    /// What ran in it.
    pub agent: AgentKind,
    /// The verdict when the session was closed or Sakshi quit.
    pub trust: TrustReport,
}

impl SessionVerdict {
    /// Whether the session belongs in the exit summary: every agent does, a
    /// plain shell only when something in it looked off.
    fn is_worth_saying(&self) -> bool {
        self.agent != AgentKind::Generic || self.trust.badge != Badge::Green
    }

    /// One line: the badge, the session, and the finding that decided it.
    fn line(&self, name_width: usize) -> String {
        let badge = self.trust.badge.as_str().to_uppercase();
        let why = match self.trust.worst() {
            None => "nothing looked off".to_string(),
            Some(worst) => {
                let others = self
                    .trust
                    .findings
                    .iter()
                    .filter(|f| f.severity > crate::trust::Severity::Info)
                    .count()
                    - 1;
                let more = if others == 0 {
                    String::new()
                } else {
                    format!(" (+{others} more)")
                };
                format!(
                    "{}: {}{more}",
                    worst.concern().title(),
                    one_line(&worst.reason)
                )
            }
        };
        format!("  {badge:<6}  {:<name_width$}  {why}", self.name)
    }
}

/// Longest reason shown in the exit summary before it is cut.
const MAX_EXIT_REASON: usize = 100;

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_EXIT_REASON {
        return flat;
    }
    let cut: String = flat.chars().take(MAX_EXIT_REASON - 3).collect();
    format!("{cut}...")
}

/// What to print when Sakshi exits: one line per session worth mentioning, so
/// the verdict is the last thing on screen. `None` when there is nothing to
/// say. `logged` is whether activity logs were written, and so whether
/// `sakshi report` can show more.
pub fn exit_summary(verdicts: &[SessionVerdict], logged: bool) -> Option<String> {
    let shown: Vec<&SessionVerdict> = verdicts.iter().filter(|v| v.is_worth_saying()).collect();
    if shown.is_empty() {
        return None;
    }
    let width = shown
        .iter()
        .map(|v| v.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::from("Sakshi's verdict:\n");
    for verdict in &shown {
        out.push_str(&verdict.line(width));
        out.push('\n');
    }
    if logged {
        match shown.as_slice() {
            [one] => out.push_str(&format!("Details: sakshi report {}", one.id)),
            _ => out.push_str("Details: sakshi sessions, then sakshi report <session>"),
        }
    } else {
        out = out.trim_end().to_string();
    }
    Some(out)
}

/// `path` without the leading `cwd`, when it is under it.
fn relative(path: &str, cwd: &str) -> String {
    let path = path.replace('\\', "/");
    let cwd = cwd.replace('\\', "/");
    let cwd = cwd.trim_end_matches('/');
    match path.strip_prefix(cwd) {
        Some(rest) if !cwd.is_empty() && rest.starts_with('/') => {
            rest.trim_start_matches('/').to_string()
        }
        _ => path,
    }
}

/// Make text safe inside a Markdown code span on one line.
fn code_safe(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "'")
}

/// One line of a replay: time since `start`, then what happened.
pub fn timeline_line(event: &Event, start: OffsetDateTime) -> String {
    let secs = (event.ts - start).whole_seconds().max(0);
    let stamp = format!(
        "+{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    );
    let body = match &event.kind {
        EventKind::Session { name, agent, cwd } => {
            format!("session started: {name} ({agent}) in {cwd}")
        }
        EventKind::Trust { badge, findings } => {
            let worst = findings
                .iter()
                .max_by_key(|f| f.severity)
                .map(|f| format!(" - {}", f.reason))
                .unwrap_or_default();
            format!(
                "trust: {} ({} finding(s)){worst}",
                badge.as_str().to_uppercase(),
                findings.len()
            )
        }
        other => Event {
            session_id: event.session_id,
            ts: event.ts,
            kind: other.clone(),
        }
        .describe()
        .unwrap_or_default(),
    };
    format!("{stamp}  {body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::{Badge, Finding, Severity, TrustReport};

    fn at(secs: i64, kind: EventKind) -> Event {
        Event {
            session_id: SessionId(0xab),
            ts: OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap(),
            kind,
        }
    }

    fn session_event() -> Event {
        at(
            0,
            EventKind::Session {
                name: "claude".into(),
                agent: "claude".into(),
                cwd: "/work/app".into(),
            },
        )
    }

    fn sample() -> Vec<Event> {
        vec![
            session_event(),
            at(1, EventKind::State(SessionState::Working)),
            at(
                5,
                EventKind::FileEdit {
                    path: "src/a.rs".into(),
                },
            ),
            at(
                6,
                EventKind::Command {
                    command: "git push --force".into(),
                },
            ),
            at(125, EventKind::State(SessionState::Done)),
        ]
    }

    #[test]
    fn pr_markdown_is_short_relative_and_free_of_local_ids() {
        let mut events = sample();
        events.push(at(
            7,
            EventKind::FileEdit {
                path: "/work/app/src/b.rs".into(),
            },
        ));
        events.push(at(
            8,
            EventKind::FileEdit {
                path: "/elsewhere/c.rs".into(),
            },
        ));
        events.push(at(
            9,
            EventKind::TestRun {
                command: "cargo test".into(),
                passed: Some(true),
            },
        ));
        let md = Report::from_events(&events).unwrap().to_pr_markdown();
        for expected in [
            "### Agent changes (recorded by Sakshi)",
            "**Trust: RED**",
            "`destructive-command`",
            "**Tests (run by the agent):** 1 passed, 0 failed, 0 unknown.",
            "**Files edited (3):**",
            "- `src/a.rs`",
            "- `src/b.rs`",
            "- `/elsewhere/c.rs`",
        ] {
            assert!(md.contains(expected), "missing {expected:?} in:\n{md}");
        }
        // Nothing machine-specific: no session id, directory or timestamp.
        for absent in ["/work/app", "Started", "Session:"] {
            assert!(!md.contains(absent), "found {absent:?} in:\n{md}");
        }
    }

    #[test]
    fn pr_markdown_for_a_quiet_session_says_so() {
        let events = vec![
            session_event(),
            at(1, EventKind::State(SessionState::Working)),
        ];
        let md = Report::from_events(&events).unwrap().to_pr_markdown();
        assert!(md.contains("Nothing looked off."));
        assert!(md.contains("**Tests:** the agent ran none."));
        assert!(!md.contains("Files edited"));
    }

    #[test]
    fn an_empty_log_has_no_report() {
        assert_eq!(Report::from_events(&[]), None);
    }

    #[test]
    fn reads_metadata_and_totals() {
        let r = Report::from_events(&sample()).unwrap();
        assert_eq!((r.name.as_str(), r.agent.as_str()), ("claude", "claude"));
        assert_eq!(r.cwd, "/work/app");
        assert_eq!(r.session_id, SessionId(0xab));
        assert_eq!(r.duration_seconds(), 125);
        assert_eq!(r.final_state, Some(SessionState::Done));
        assert_eq!(r.event_count, 5);
        assert_eq!(r.summary.files_edited, ["src/a.rs"]);
    }

    #[test]
    fn without_a_recorded_verdict_trust_is_computed_from_events() {
        let r = Report::from_events(&sample()).unwrap();
        assert_eq!(r.trust.badge, Badge::Red);
    }

    #[test]
    fn a_recorded_verdict_wins_because_it_knows_about_diffs() {
        let mut events = sample();
        events.push(at(
            126,
            EventKind::Trust {
                badge: Badge::Yellow,
                findings: vec![Finding {
                    rule: "tests-skipped".into(),
                    severity: Severity::Warn,
                    reason: "added a skip".into(),
                }],
            },
        ));
        let r = Report::from_events(&events).unwrap();
        assert_eq!(r.trust.badge, Badge::Yellow);
        assert_eq!(r.trust.findings.len(), 1);
    }

    #[test]
    fn logs_without_a_session_record_still_report() {
        let r = Report::from_events(&sample()[1..]).unwrap();
        assert_eq!(r.name, SessionId(0xab).to_string());
        assert_eq!(r.agent, "unknown");
    }

    #[test]
    fn markdown_has_the_expected_sections() {
        let md = Report::from_events(&sample()).unwrap().to_markdown();
        for expected in [
            "# Sakshi report: claude",
            "- Session: `00000000000000ab`",
            "- Duration: 2m",
            "- Final state: done",
            "## Trust: RED",
            "**alert** `destructive-command`",
            "## What happened",
            "- Edited 1 file(s): src/a.rs",
            "## Files edited",
            "- `src/a.rs`",
            "## Commands",
            "- `git push --force`",
        ] {
            assert!(md.contains(expected), "missing {expected:?} in:\n{md}");
        }
    }

    #[test]
    fn a_quiet_clean_session_says_so() {
        let md = Report::from_events(&[session_event()])
            .unwrap()
            .to_markdown();
        assert!(md.contains("Nothing looked off."), "{md}");
        assert!(md.contains("No activity was recorded."), "{md}");
        assert!(!md.contains("## Commands"), "{md}");
    }

    #[test]
    fn long_command_lists_are_capped_and_backticks_defused() {
        let mut events = vec![session_event()];
        for i in 0..20 {
            events.push(at(
                1 + i,
                EventKind::Command {
                    command: format!("echo `date` {i}"),
                },
            ));
        }
        let md = Report::from_events(&events).unwrap().to_markdown();
        assert!(md.contains("...and 5 more"), "{md}");
        assert!(md.contains("`echo 'date' 0`"), "{md}");
    }

    #[test]
    fn timeline_lines_show_elapsed_time_and_every_kind() {
        let start = sample()[0].ts;
        let line = |e: &Event| timeline_line(e, start);
        let events = sample();
        assert_eq!(
            line(&events[0]),
            "+00:00:00  session started: claude (claude) in /work/app"
        );
        assert_eq!(line(&events[2]), "+00:00:05  edit src/a.rs");
        assert_eq!(line(&events[4]), "+00:02:05  state: done");
        let trust = at(
            3700,
            EventKind::Trust {
                badge: Badge::Red,
                findings: vec![Finding {
                    rule: "r".into(),
                    severity: Severity::Alert,
                    reason: "bad thing".into(),
                }],
            },
        );
        assert_eq!(
            line(&trust),
            "+01:01:40  trust: RED (1 finding(s)) - bad thing"
        );
    }

    #[test]
    fn findings_are_grouped_under_the_question_they_answer() {
        let mut events = sample();
        events.push(at(
            126,
            EventKind::Trust {
                badge: Badge::Red,
                findings: vec![
                    Finding {
                        rule: "destructive-command".into(),
                        severity: Severity::Alert,
                        reason: "ran `git push --force`".into(),
                    },
                    Finding {
                        rule: "tests-skipped".into(),
                        severity: Severity::Warn,
                        reason: "added a skip".into(),
                    },
                ],
            },
        ));
        let report = Report::from_events(&events).unwrap();
        let md = report.to_markdown();
        let faked = md.find("### Faked success").unwrap();
        let risky = md.find("### Risky actions").unwrap();
        // Concern order, not severity order, decides which group comes first.
        assert!(faked < risky, "{md}");
        assert!(md[faked..risky].contains("`tests-skipped`"), "{md}");
        assert!(md[risky..].contains("`destructive-command`"), "{md}");
        let pr = report.to_pr_markdown();
        assert!(pr.contains("**Faked success**\n\n- **warn**"), "{pr}");
        assert!(!pr.contains("### Faked"), "{pr}");
    }

    fn verdict(name: &str, agent: AgentKind, findings: Vec<Finding>) -> SessionVerdict {
        SessionVerdict {
            id: SessionId(0xab),
            name: name.into(),
            agent,
            trust: TrustReport {
                badge: Badge::from_findings(&findings),
                findings,
            },
        }
    }

    fn finding(rule: &str, severity: Severity, reason: &str) -> Finding {
        Finding {
            rule: rule.into(),
            severity,
            reason: reason.into(),
        }
    }

    #[test]
    fn the_exit_summary_names_each_agent_and_why() {
        let verdicts = [
            verdict(
                "claude",
                AgentKind::Claude,
                vec![
                    finding("tests-deleted", Severity::Alert, "deleted tests/a.py"),
                    finding("tests-skipped", Severity::Warn, "added\n a skip"),
                    finding("lockfile-changed", Severity::Info, "Cargo.lock changed"),
                ],
            ),
            verdict("codex-2", AgentKind::Codex, Vec::new()),
        ];
        let text = exit_summary(&verdicts, true).unwrap();
        assert_eq!(
            text,
            "Sakshi's verdict:\n\
             \x20 RED     claude   Faked success: deleted tests/a.py (+1 more)\n\
             \x20 GREEN   codex-2  nothing looked off\n\
             Details: sakshi sessions, then sakshi report <session>"
        );
    }

    #[test]
    fn the_exit_summary_stays_quiet_about_clean_shells() {
        let shell = verdict("zsh", AgentKind::Generic, Vec::new());
        assert_eq!(exit_summary(std::slice::from_ref(&shell), true), None);
        assert_eq!(exit_summary(&[], true), None);
        // A note alone does not make a shell worth mentioning.
        let noted = verdict(
            "zsh",
            AgentKind::Generic,
            vec![finding("lockfile-changed", Severity::Info, "x")],
        );
        assert_eq!(exit_summary(&[noted], true), None);
        // A shell where something looked off is, and one session gets its id.
        let risky = verdict(
            "zsh",
            AgentKind::Generic,
            vec![finding(
                "destructive-command",
                Severity::Alert,
                "ran `rm -rf /`",
            )],
        );
        let text = exit_summary(&[shell, risky], true).unwrap();
        assert!(
            text.contains("RED     zsh  Risky actions: ran `rm -rf /`"),
            "{text}"
        );
        assert!(
            text.ends_with("Details: sakshi report 00000000000000ab"),
            "{text}"
        );
    }

    #[test]
    fn the_exit_summary_does_not_point_at_logs_that_were_not_written() {
        let v = verdict("claude", AgentKind::Claude, Vec::new());
        let text = exit_summary(&[v], false).unwrap();
        assert_eq!(
            text,
            "Sakshi's verdict:\n  GREEN   claude  nothing looked off"
        );
    }

    #[test]
    fn long_reasons_are_cut_in_the_exit_summary() {
        let long = "x".repeat(300);
        let v = verdict(
            "claude",
            AgentKind::Claude,
            vec![finding("sensitive-file", Severity::Warn, &long)],
        );
        let text = exit_summary(&[v], false).unwrap();
        let line = text.lines().nth(1).unwrap();
        assert!(line.ends_with("..."), "{line}");
        assert!(line.chars().count() < 140, "{line}");
    }
}
