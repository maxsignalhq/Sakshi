//! Reports and replays built from a session's recorded events.

use time::OffsetDateTime;

use crate::event::{format_age, Event, EventKind};
use crate::session::{SessionId, SessionState};
use crate::summary::{summarize, Summary};
use crate::trust::{evaluate, TrustReport};

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
        if self.trust.findings.is_empty() {
            out.push_str("Nothing looked off.\n\n");
        }
        for finding in &self.trust.findings {
            out.push_str(&format!(
                "- **{}** `{}`: {}\n",
                finding.severity.as_str(),
                finding.rule,
                finding.reason
            ));
        }
        if !self.trust.findings.is_empty() {
            out.push('\n');
        }

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
    use crate::trust::{Badge, Finding, Severity};

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
}
